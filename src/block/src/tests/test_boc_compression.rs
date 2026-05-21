/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use crate::{BuilderData, IBitstring, Serializable};

// ============================================================================
// BOC COMPRESSION TEST SUITE
// ============================================================================
//
// This test suite validates wire compatibility between the Rust and C++
// implementations of ImprovedStructureLZ4 compression.
//
// Test categories:
// 1. Basic functionality tests
// 2. Round-trip tests (compress → decompress → verify)
// 3. RFC test vectors (byte-exact compatibility)
// 4. Edge cases (special cells, bit alignment, large data)
// 5. MerkleUpdate depth-balance reconstruction
//
// C++ Reference: docs/ton-node-cpp/crypto/vm/boc-compression.cpp
// ============================================================================

// ============================================
// Tests for boc_compress_improved_structure_lz4
// ============================================

#[test]
fn test_compress_empty_roots_fails() {
    let result = boc_compress_improved_structure_lz4(vec![]);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("No root cells were provided"));
}

#[test]
fn test_compress_single_cell() {
    let cell = build_single_cell(0x12345678);
    let result = boc_compress_improved_structure_lz4(vec![cell]);
    assert!(result.is_ok());
    let compressed = result.unwrap();
    assert!(!compressed.is_empty());
}

#[test]
fn test_compress_simple_tree() {
    let root = build_simple_tree();
    let result = boc_compress_improved_structure_lz4(vec![root]);
    assert!(result.is_ok());
    let compressed = result.unwrap();
    assert!(!compressed.is_empty());
}

#[test]
fn test_compress_multiple_roots() {
    let root1 = build_single_cell(1);
    let root2 = build_single_cell(2);
    let root3 = build_simple_tree();

    let result = boc_compress_improved_structure_lz4(vec![root1, root2, root3]);
    assert!(result.is_ok());
    let compressed = result.unwrap();
    assert!(!compressed.is_empty());
}

#[test]
fn test_compress_tree_with_large_data() {
    let root = build_tree_with_large_data();
    let result = boc_compress_improved_structure_lz4(vec![root]);
    assert!(result.is_ok());
}

#[test]
fn test_compress_deep_tree() {
    let root = build_deep_tree(50);
    let result = boc_compress_improved_structure_lz4(vec![root]);
    assert!(result.is_ok());
}

#[test]
fn test_compress_dag_with_shared_references() {
    let root = build_dag_tree();
    let result = boc_compress_improved_structure_lz4(vec![root]);
    assert!(result.is_ok());
}

#[test]
fn test_compress_default_cell() {
    let cell = Cell::default();
    let result = boc_compress_improved_structure_lz4(vec![cell]);
    assert!(result.is_ok());
}

// ============================================
// Tests for boc_decompress_improved_structure_lz4
// ============================================

#[test]
fn test_decompress_empty_input_fails() {
    let result = boc_decompress_improved_structure_lz4(vec![], 1024);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("input too small"));
}

#[test]
fn test_decompress_input_too_small_for_header() {
    // Less than 4 bytes (K_DECOMPRESSED_SIZE)
    let result = boc_decompress_improved_structure_lz4(vec![0, 1, 2], 1024);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("input too small"));
}

#[test]
fn test_decompress_size_exceeds_max() {
    // Create header with large decompressed size
    let large_size: u32 = 10_000_000;
    let mut data = large_size.to_be_bytes().to_vec();
    data.extend_from_slice(&[0u8; 10]); // Some payload

    let result = boc_decompress_improved_structure_lz4(data, 1024);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid decompressed size"));
}

// ============================================
// Round-trip tests (compress then decompress)
// ============================================

#[test]
fn test_roundtrip_single_cell() {
    let original = build_single_cell(0x12345678);
    let compressed = boc_compress_improved_structure_lz4(vec![original.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();

    assert_eq!(decompressed.len(), 1, "Expected 1 root cell after decompression");
    assert_eq!(decompressed[0], original, "Decompressed cell doesn't match original");
}

#[test]
fn test_roundtrip_simple_tree() {
    let original = build_simple_tree();
    let compressed = boc_compress_improved_structure_lz4(vec![original.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();

    assert_eq!(decompressed.len(), 1, "Expected 1 root cell after decompression");
    assert_eq!(decompressed[0], original, "Decompressed tree doesn't match original");
}

#[test]
fn test_roundtrip_multiple_roots() {
    let root1 = build_single_cell(1);
    let root2 = build_single_cell(2);
    let root3 = build_simple_tree();
    let originals = vec![root1.clone(), root2.clone(), root3.clone()];

    let compressed = boc_compress_improved_structure_lz4(originals.clone()).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();

    assert_eq!(decompressed.len(), 3, "Expected 3 root cells after decompression");
    assert_eq!(decompressed[0], root1, "First root doesn't match");
    assert_eq!(decompressed[1], root2, "Second root doesn't match");
    assert_eq!(decompressed[2], root3, "Third root doesn't match");
}

#[test]
fn test_roundtrip_dag_structure() {
    let original = build_dag_tree();
    let compressed = boc_compress_improved_structure_lz4(vec![original.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();

    assert_eq!(decompressed.len(), 1, "Expected 1 root cell after decompression");
    assert_eq!(decompressed[0], original, "Decompressed DAG doesn't match original");
}

#[test]
fn test_roundtrip_deep_tree() {
    let original = build_deep_tree(100);
    let compressed = boc_compress_improved_structure_lz4(vec![original.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();

    assert_eq!(decompressed.len(), 1, "Expected 1 root cell after decompression");
    assert_eq!(decompressed[0], original, "Decompressed deep tree doesn't match original");
}

#[test]
fn test_roundtrip_large_data_cell() {
    let original = build_tree_with_large_data();
    let compressed = boc_compress_improved_structure_lz4(vec![original.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();

    assert_eq!(decompressed.len(), 1, "Expected 1 root cell after decompression");
    assert_eq!(decompressed[0], original, "Decompressed large data cell doesn't match original");
}

#[test]
fn test_roundtrip_default_cell() {
    let original = Cell::default();
    let compressed = boc_compress_improved_structure_lz4(vec![original.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();

    assert_eq!(decompressed.len(), 1, "Expected 1 root cell after decompression");
    assert_eq!(decompressed[0], original, "Decompressed default cell doesn't match original");
}

// ============================================
// Tests for build_graph_recursive (via compression)
// ============================================

#[test]
fn test_graph_handles_max_references() {
    // Create a cell with 4 references (maximum allowed)
    let c1 = build_single_cell(1);
    let c2 = build_single_cell(2);
    let c3 = build_single_cell(3);
    let c4 = build_single_cell(4);

    let mut root = BuilderData::new();
    root.append_u32(0).unwrap();
    root.checked_append_reference(c1).unwrap();
    root.checked_append_reference(c2).unwrap();
    root.checked_append_reference(c3).unwrap();
    root.checked_append_reference(c4).unwrap();

    let root_cell = root.into_cell().unwrap();
    let result = boc_compress_improved_structure_lz4(vec![root_cell]);
    assert!(result.is_ok());
}

#[test]
fn test_graph_deduplicates_shared_cells() {
    // Build a DAG where the same cell is referenced multiple times
    let shared = build_single_cell(0xDEAD);

    let mut c1 = BuilderData::new();
    c1.append_u32(1).unwrap();
    c1.checked_append_reference(shared.clone()).unwrap();

    let mut c2 = BuilderData::new();
    c2.append_u32(2).unwrap();
    c2.checked_append_reference(shared.clone()).unwrap();

    let mut c3 = BuilderData::new();
    c3.append_u32(3).unwrap();
    c3.checked_append_reference(shared).unwrap();

    let mut root = BuilderData::new();
    root.append_u32(0).unwrap();
    root.checked_append_reference(c1.into_cell().unwrap()).unwrap();
    root.checked_append_reference(c2.into_cell().unwrap()).unwrap();
    root.checked_append_reference(c3.into_cell().unwrap()).unwrap();

    let root_cell = root.into_cell().unwrap();

    // This should succeed and the shared cell should only appear once in the graph
    let result = boc_compress_improved_structure_lz4(vec![root_cell]);
    assert!(result.is_ok());
}

// ============================================
// Integration tests with boc_decompress
// ============================================

#[test]
fn test_boc_decompress_invalid_algorithm() {
    // Algorithm byte 255 is invalid
    let data = vec![255u8, 0, 0, 0, 0];
    let result = boc_decompress(&data, 1024);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Invalid compression algorithm"));
}

#[test]
fn test_boc_decompress_zero_max_size() {
    let data = vec![1u8, 0, 0, 0, 0];
    let result = boc_decompress(&data, 0);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Can't decompress empty data"));
}

// ============================================
// MAX_IMPROVED_NODE_COUNT regression tests
// ============================================

/// Builds `n` distinct leaf cells so the compressor's DAG contains exactly
/// `n` nodes.
fn build_n_distinct_leaves(n: usize) -> Vec<Cell> {
    (0..n)
        .map(|i| {
            let mut b = BuilderData::new();
            b.append_u64(i as u64).unwrap();
            b.into_cell().unwrap()
        })
        .collect()
}

/// Crafts a minimal ImprovedStructureLZ4 payload whose header advertises the
/// given `node_count`. The rest of the payload is intentionally truncated:
/// the tests below only assert behavior of the node-count cap check, which
/// fires (or does not fire) before any further parsing.
fn craft_improved_lz4_with_node_count(node_count: u32) -> Vec<u8> {
    let mut inner: Vec<u8> = Vec::new();
    inner.extend_from_slice(&1u32.to_be_bytes()); // root_count = 1
    inner.extend_from_slice(&0u32.to_be_bytes()); // root_indexes[0] = 0
    inner.extend_from_slice(&node_count.to_be_bytes());
    let mut compressed = lz4::block::compress(&inner, None, true).unwrap();
    compressed[0..4].copy_from_slice(&(inner.len() as u32).to_be_bytes());
    compressed
}

#[test]
fn test_decompress_node_count_just_over_limit_fails() {
    let payload = craft_improved_lz4_with_node_count((MAX_IMPROVED_NODE_COUNT + 1) as u32);
    let err = boc_decompress_improved_structure_lz4(payload, 16 << 20).unwrap_err().to_string();
    assert!(err.contains("exceeds limit"), "unexpected error: {err}");
}

#[test]
fn test_decompress_node_count_at_limit_does_not_trigger_cap_check() {
    // node_count exactly at the limit must NOT trigger the cap check. The
    // crafted payload is truncated, so decompression still fails — but for
    // unrelated reasons. The assertion only forbids the cap-check error.
    let payload = craft_improved_lz4_with_node_count(MAX_IMPROVED_NODE_COUNT as u32);
    let err = boc_decompress_improved_structure_lz4(payload, 16 << 20).unwrap_err().to_string();
    assert!(!err.contains("exceeds limit"), "must not fail on node count cap: {err}");
}

#[test]
fn test_compress_node_count_just_over_limit_fails() {
    let roots = build_n_distinct_leaves(MAX_IMPROVED_NODE_COUNT + 1);
    let err = boc_compress_improved_structure_lz4(roots).unwrap_err().to_string();
    assert!(err.contains("exceeds limit"), "unexpected error: {err}");
}

#[test]
fn test_compress_node_count_at_limit_passes() {
    let roots = build_n_distinct_leaves(MAX_IMPROVED_NODE_COUNT);
    let result = boc_compress_improved_structure_lz4(roots);
    assert!(result.is_ok(), "expected success at the limit, got: {:?}", result.err());
}

// ============================================
// Test vectors from RFC documentation
// ============================================

/// Vector 1: Single Leaf Cell (32 bits) - "TEST"
#[test]
fn test_vector_1_single_leaf_cell() {
    // Single cell with 32 bits of data: 0x54455354 (ASCII "TEST")
    let mut builder = BuilderData::new();
    builder.append_raw(&[0x54, 0x45, 0x53, 0x54], 32).unwrap();
    let cell = builder.into_cell().unwrap();

    // Verify hash
    let hash = cell.repr_hash();
    assert_eq!(
        hex::encode(hash.as_slice()),
        "0decf040ee6032aca37e26b59a070ef0af033ea91abc2bbdecf8b879d4ce1e57"
    );

    // Test round-trip
    let compressed = boc_compress_improved_structure_lz4(vec![cell.clone()]).unwrap();
    println!("Compressed data:   {}", hex::encode(&compressed));
    println!(
        "DeCompressed data: {}",
        hex::encode(boc_decompress_baseline_lz4(compressed.clone(), 1024).unwrap())
    );
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    println!("Cell data: {}", hex::encode(decompressed[0].data()));
    assert_eq!(decompressed[0], cell);
    let hash = decompressed[0].repr_hash();
    assert_eq!(
        hex::encode(hash.as_slice()),
        "0decf040ee6032aca37e26b59a070ef0af033ea91abc2bbdecf8b879d4ce1e57"
    );
}

// Ton encoded cell compatibility test
#[test]
fn test_vector_1_ton_encoded() {
    let compressed = hex::decode("000000125200000001000100700100a054455354").unwrap();
    println!("Compressed data:   {}", hex::encode(&compressed));
    println!(
        "DeCompressed data: {}",
        hex::encode(boc_decompress_baseline_lz4(compressed.clone(), 1024).unwrap())
    );

    let decompressed = boc_decompress_improved_structure_lz4(compressed.clone(), 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    println!("Cell data: {}", hex::encode(decompressed[0].data()));
    assert_eq!(decompressed[0].data(), "TEST".as_bytes());
    let hash = decompressed[0].repr_hash();
    assert_eq!(
        hex::encode(hash.as_slice()),
        "0decf040ee6032aca37e26b59a070ef0af033ea91abc2bbdecf8b879d4ce1e57"
    );

    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 2: Parent-Child (Two Cells) - RFC encoded sample (C++ canonical output)
#[test]
fn test_vector_2_parent_child_ton_encoded() {
    let compressed = hex::decode("000000195200000001000100e00201a000a080cafebabedeadbeef").unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 3: Two Root Cells - RFC encoded sample (C++ canonical output)
#[test]
fn test_vector_3_two_roots_ton_encoded() {
    let compressed =
        hex::decode("0000001c920000000200000001000100d00200a000a02222222211111111").unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 2);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 4: Deep Chain (4 Levels) - RFC encoded sample (C++ canonical output)
#[test]
fn test_vector_4_deep_chain_ton_encoded() {
    let compressed = hex::decode(
        "000000255200000001000100300401a00200f00400a0e011111111222222223333333344444444",
    )
    .unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 5: Fan-Out (1 Parent, 4 Children) - RFC encoded sample
#[test]
fn test_vector_5_fan_out_ton_encoded() {
    let compressed = hex::decode(
        "0000002c5200000001000100430504a0000200f0071b00bbbbbbbbaa000303aa000202aa000101aa000000",
    )
    .unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 6: DAG with Shared Cell - RFC encoded sample
#[test]
fn test_vector_6_dag_shared_ton_encoded() {
    let compressed = hex::decode(
        "000000255200000001000100f00b0402a001a001a000a090500f0000aaaa0001bbbb00025a5a5a00",
    )
    .unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 7: Large Data Cell (256 bits) - RFC encoded sample
#[test]
fn test_vector_7_large_data_256bits_ton_encoded() {
    let compressed =
        hex::decode("0000002f52000000010001008f01002101deadbeef04000450efdeadbeef").unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 8: Non-Byte-Aligned Data (37 bits) - RFC encoded sample
#[test]
fn test_vector_8_non_byte_aligned_37bits_ton_encoded() {
    let compressed = hex::decode("000000135200000001000100800100a5a879bdffff").unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 9: Empty Data Cell with Reference - RFC encoded sample (C++ canonical output)
#[test]
fn test_vector_9_empty_data_with_ref_ton_encoded() {
    let compressed = hex::decode("000000155200000001000100a002018000a08012345678").unwrap();
    let decompressed =
        boc_decompress_improved_structure_lz4(compressed.clone(), 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed = boc_compress_improved_structure_lz4(decompressed).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 10: Generic API with Algorithm Prefix - RFC encoded sample
#[test]
fn test_vector_10_algorithm_prefix_ton_encoded() {
    let compressed = hex::decode("01000000125200000001000100700100a0abcdef01").unwrap();
    let decompressed = boc_decompress(&compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    let recompressed =
        boc_compress(decompressed, CompressionAlgorithm::ImprovedStructureLZ4).unwrap();
    assert_eq!(recompressed, compressed);
}

/// Vector 2: Parent-Child (Two Cells)
#[test]
fn test_vector_2_parent_child() {
    // Child cell: 32 bits data 0xDEADBEEF, 0 references
    let mut child_builder = BuilderData::new();
    child_builder.append_raw(&[0xDE, 0xAD, 0xBE, 0xEF], 32).unwrap();
    let child = child_builder.into_cell().unwrap();

    // Parent cell: 32 bits data 0xCAFEBABE, 1 reference
    let mut parent_builder = BuilderData::new();
    parent_builder.append_raw(&[0xCA, 0xFE, 0xBA, 0xBE], 32).unwrap();
    parent_builder.checked_append_reference(child).unwrap();
    let parent = parent_builder.into_cell().unwrap();

    // Verify parent hash
    let hash = parent.repr_hash();
    assert_eq!(
        hex::encode(hash.as_slice()),
        "089c7ac0a421a928910fc8e1c10921ed7a1ac7997ed209f98285237e7004052c"
    );

    // Test round-trip
    let compressed = boc_compress_improved_structure_lz4(vec![parent.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], parent);
}

/// Vector 3: Two Root Cells
#[test]
fn test_vector_3_two_roots() {
    // Root 1: 32 bits data 0x11111111, 0 references
    let mut root1_builder = BuilderData::new();
    root1_builder.append_raw(&[0x11, 0x11, 0x11, 0x11], 32).unwrap();
    let root1 = root1_builder.into_cell().unwrap();

    // Root 2: 32 bits data 0x22222222, 0 references
    let mut root2_builder = BuilderData::new();
    root2_builder.append_raw(&[0x22, 0x22, 0x22, 0x22], 32).unwrap();
    let root2 = root2_builder.into_cell().unwrap();

    // Test round-trip with multiple roots
    let compressed =
        boc_compress_improved_structure_lz4(vec![root1.clone(), root2.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024).unwrap();
    assert_eq!(decompressed.len(), 2);
    assert_eq!(decompressed[0], root1);
    assert_eq!(decompressed[1], root2);
}

/// Vector 4: Deep Chain (4 Levels)
#[test]
fn test_vector_4_deep_chain() {
    // Build from bottom up
    // Leaf: 32 bits 0x44444444
    let mut leaf_builder = BuilderData::new();
    leaf_builder.append_raw(&[0x44, 0x44, 0x44, 0x44], 32).unwrap();
    let leaf = leaf_builder.into_cell().unwrap();

    // Level2: 32 bits 0x33333333 -> Leaf
    let mut level2_builder = BuilderData::new();
    level2_builder.append_raw(&[0x33, 0x33, 0x33, 0x33], 32).unwrap();
    level2_builder.checked_append_reference(leaf).unwrap();
    let level2 = level2_builder.into_cell().unwrap();

    // Level1: 32 bits 0x22222222 -> Level2
    let mut level1_builder = BuilderData::new();
    level1_builder.append_raw(&[0x22, 0x22, 0x22, 0x22], 32).unwrap();
    level1_builder.checked_append_reference(level2).unwrap();
    let level1 = level1_builder.into_cell().unwrap();

    // Root: 32 bits 0x11111111 -> Level1
    let mut root_builder = BuilderData::new();
    root_builder.append_raw(&[0x11, 0x11, 0x11, 0x11], 32).unwrap();
    root_builder.checked_append_reference(level1).unwrap();
    let root = root_builder.into_cell().unwrap();

    // Test round-trip
    let compressed = boc_compress_improved_structure_lz4(vec![root.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], root);
}

/// Vector 9: Empty Data Cell with Reference
#[test]
fn test_vector_9_empty_data_with_ref() {
    // Child cell: 32 bits 0x12345678, 0 refs
    let mut child_builder = BuilderData::new();
    child_builder.append_raw(&[0x12, 0x34, 0x56, 0x78], 32).unwrap();
    let child = child_builder.into_cell().unwrap();

    // Parent cell: 0 bits data, 1 reference
    let mut parent_builder = BuilderData::new();
    parent_builder.checked_append_reference(child).unwrap();
    let parent = parent_builder.into_cell().unwrap();

    // Test round-trip
    let compressed = boc_compress_improved_structure_lz4(vec![parent.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], parent);
}

// ============================================
// Additional coverage for tricky bit-level paths
// ============================================

#[test]
fn test_roundtrip_non_small_non_byte_aligned_cell() {
    // Ensure non-small cells may have non-byte-aligned bit lengths (>= 128 bits, rem != 0).
    // This exercises the padding+marker path in both encode and decode.
    let mut builder = BuilderData::new();
    let data = vec![0xA5u8; 17]; // 136 bits available
    builder.append_raw(&data, 129).unwrap(); // 129 bits => non-small and not byte-aligned
    let cell = builder.into_cell().unwrap();

    let compressed = boc_compress_improved_structure_lz4(vec![cell.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], cell);
}

#[test]
fn test_decompress_depth_balance_marker_ct9_under_merkle_update() {
    // This crafts a valid inner stream that contains a C++-style depth-balance marker (cell_type = 9)
    // under a MerkleUpdate right subtree, and verifies that the decoder reconstructs the missing coins.
    //
    // Layout (topological ranks are literal indices here):
    //   0: MerkleUpdate (special, refs -> 1(old), 4(new_marker))
    //   1: old_root (DepthBalanceInfo coins=100, refs -> 2, 3)
    //   2: old_leaf1 (coins=30)
    //   3: old_leaf2 (coins=70)
    //   4: new_root MARKER (cell_type=9, no payload on wire, refs -> 5, 6)
    //   5: new_leaf1 (coins=40)
    //   6: new_leaf2 (coins=60)
    //
    // Child diffs: (40-30) + (60-70) = 0, so new_root coins can be reconstructed as 100 + 0 = 100.
    fn build_depth_balance_cell(coins: u128, refs: &[Cell]) -> Cell {
        let mut b = BuilderData::new();
        write_depth_balance_coins(&mut b, coins).unwrap();
        for r in refs {
            b.checked_append_reference(r.clone()).unwrap();
        }
        b.into_cell().unwrap()
    }

    fn cell_bits(cell: &Cell) -> (Vec<u8>, usize) {
        let bit_len = cell.bit_length();
        let bytes_len = (bit_len + 7) / 8;
        let mut bytes = cell.data()[..bytes_len].to_vec();
        let rem = bit_len % 8;
        if rem != 0 {
            let mask = 0xFFu8 << (8 - rem);
            if let Some(last) = bytes.last_mut() {
                *last &= mask;
            }
        }
        (bytes, bit_len)
    }

    // Build old/new depth-balance trees
    let old_leaf1 = build_depth_balance_cell(30, &[]);
    let old_leaf2 = build_depth_balance_cell(70, &[]);
    let old_root = build_depth_balance_cell(100, &[old_leaf1.clone(), old_leaf2.clone()]);

    let new_leaf1 = build_depth_balance_cell(40, &[]);
    let new_leaf2 = build_depth_balance_cell(60, &[]);
    let new_root_full = build_depth_balance_cell(100, &[new_leaf1.clone(), new_leaf2.clone()]);

    // Build a MerkleUpdate cell that *directly* references (old_root, new_root_full).
    // We avoid `MerkleUpdate::create` here because it may produce pruned/proof subtrees, which would
    // change children level masks and thus the MerkleUpdate cell's `repr_hash()`.
    let mu_cell = {
        let mut b = BuilderData::new();
        b.set_type(CellType::MerkleUpdate);
        b.append_u8(u8::from(CellType::MerkleUpdate)).unwrap();
        old_root.hash(0).write_to(&mut b).unwrap();
        new_root_full.hash(0).write_to(&mut b).unwrap();
        b.append_u16(old_root.depth(0)).unwrap();
        b.append_u16(new_root_full.depth(0)).unwrap();
        b.checked_append_reference(old_root.clone()).unwrap();
        b.checked_append_reference(new_root_full.clone()).unwrap();
        b.into_cell().unwrap()
    };

    // Node table for indices 0..7 (None = depth-balance marker payload is elided on the wire)
    let node_count = 7usize;
    let cells: Vec<Option<Cell>> = vec![
        Some(mu_cell.clone()),
        Some(old_root.clone()),
        Some(old_leaf1.clone()),
        Some(old_leaf2.clone()),
        None, // ct=9 marker for new_root_full
        Some(new_leaf1.clone()),
        Some(new_leaf2.clone()),
    ];

    // Adjacency by topological rank (only first `refs_cnt[i]` entries are used)
    let refs_cnt = [2usize, 2, 0, 0, 2, 0, 0];
    let mut graph = vec![[0usize; 4]; node_count];
    graph[0][0] = 1;
    graph[0][1] = 4;
    graph[1][0] = 2;
    graph[1][1] = 3;
    graph[4][0] = 5;
    graph[4][1] = 6;

    // Per-node 4-bit `cell_type` field in the wire format
    // - 1: "special" (exact special type is inferred from the first byte tag)
    // - 9: depth-balance marker (payload elided and reconstructed during decode)
    let cell_type_field = [1u8, 0, 0, 0, 9, 0, 0];

    // Precompute raw data bits for all non-marker nodes
    let mut data_bytes: Vec<Vec<u8>> = vec![Vec::new(); node_count];
    let mut data_bit_len: Vec<usize> = vec![0; node_count];
    for i in 0..node_count {
        if let Some(c) = &cells[i] {
            let (bytes, bits) = cell_bits(c);
            data_bytes[i] = bytes;
            data_bit_len[i] = bits;
        }
    }

    // Compute small flags as per the format (bits < 128)
    let mut is_data_small = [false; 7];
    for i in 0..node_count {
        if cell_type_field[i] == 9 {
            continue;
        }
        is_data_small[i] = data_bit_len[i] < 128;
    }

    // Build inner serialized bitstream (MSB-first) following the C++ format.
    let mut out = BitStringWriter::with_capacity_bits(4096);
    out.push_uint(1, 32); // root_count
    out.push_uint(0, 32); // root_index[0]
    out.push_uint(node_count as u64, 32);

    // Cell metadata section
    for i in 0..node_count {
        out.push_uint(cell_type_field[i] as u64, 4);
        out.push_uint(refs_cnt[i] as u64, 4);

        if cell_type_field[i] == 9 {
            continue; // depth-balance marker: no length field
        }

        if is_data_small[i] {
            out.push_uint(1, 1);
            out.push_uint(data_bit_len[i] as u64, 7);
        } else {
            out.push_uint(0, 1);
            let bytes = data_bit_len[i] / 8; // floor
            out.push_uint((1 + bytes) as u64, 7);
        }
    }

    // Edge bitmap section
    for i in 0..node_count {
        for j in 0..refs_cnt[i] {
            let child = graph[i][j];
            out.push_bit(child == i + 1);
        }
    }

    // Prefix bits section (only for small cells; marker is skipped)
    for i in 0..node_count {
        if cell_type_field[i] == 9 {
            continue;
        }
        if !is_data_small[i] {
            continue;
        }
        let rem = data_bit_len[i] % 8;
        out.push_bits(&data_bytes[i], 0, rem);
    }

    // Graph encoding section (delta encoding for non-direct edges)
    for i in 0..node_count {
        if node_count <= i + 3 {
            continue; // small graph optimization (missing edges default to i + 2 on decode)
        }
        let required_bits = required_bits_for_delta(node_count, i);
        for j in 0..refs_cnt[i] {
            let child = graph[i][j];
            if child <= i + 1 {
                continue;
            }
            let delta = child - i - 2;
            let cur = out.len_bits();
            let threshold = 8 - ((cur + 1) % 8) + 1;
            if required_bits < threshold {
                out.push_uint(delta as u64, required_bits);
            } else {
                let avail = 8 - ((cur + 1) % 8);
                if delta < (1usize << avail) {
                    out.push_uint(1, 1);
                    out.push_uint(delta as u64, avail);
                } else {
                    out.push_uint(0, 1);
                    out.push_uint(delta as u64, required_bits);
                }
            }
        }
    }

    // Pad to byte boundary
    while out.len_bits() % 8 != 0 {
        out.push_uint(0, 1);
    }

    // Cell data section
    for i in 0..node_count {
        if cell_type_field[i] == 9 {
            continue; // payload elided
        }
        if is_data_small[i] {
            let prefix = data_bit_len[i] % 8;
            out.push_bits(&data_bytes[i], prefix, data_bit_len[i] - prefix);
        } else {
            let data_size = data_bit_len[i] + 1;
            let padding = (8 - (data_size % 8)) % 8;
            if padding != 0 {
                out.push_zeros(padding);
            }
            out.push_uint(1, 1); // marker
            out.push_bits(&data_bytes[i], 0, data_bit_len[i]);
        }
    }

    // Final padding
    while out.len_bits() % 8 != 0 {
        out.push_uint(0, 1);
    }

    let serialized = out.data;
    let mut compressed = lz4::block::compress(&serialized, None, true).unwrap();
    let size = (serialized.len() as u32).to_be_bytes();
    compressed[0..4].copy_from_slice(&size);

    let decoded = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decoded.len(), 1);

    let root = &decoded[0];
    assert_eq!(
        root.cell_type(),
        CellType::MerkleUpdate,
        "decoded root must be a MerkleUpdate cell"
    );
    assert_eq!(root.references_count(), 2, "MerkleUpdate must have 2 refs");

    let decoded_old = root.reference(0).unwrap();
    let decoded_new = root.reference(1).unwrap();

    assert_eq!(decoded_old.repr_hash(), old_root.repr_hash(), "old subtree root hash mismatch");
    assert_eq!(
        decoded_new.repr_hash(),
        new_root_full.repr_hash(),
        "new subtree root hash mismatch (ct=9 reconstruction failed)"
    );

    assert_eq!(root.bit_length(), mu_cell.bit_length(), "MerkleUpdate data bit length mismatch");
    assert_eq!(root.data(), mu_cell.data(), "MerkleUpdate raw data bytes mismatch");

    assert_eq!(root.repr_hash(), mu_cell.repr_hash(), "MerkleUpdate root hash mismatch");
}

// ============================================
// Additional edge case tests
// ============================================

/// Test roundtrip for maximum cell data length (1023 bits)
#[test]
fn test_roundtrip_max_cell_data_length() {
    let mut builder = BuilderData::new();
    // 1023 bits = 127 bytes + 7 bits (maximum allowed)
    let data = vec![0xAAu8; 128]; // 128 bytes = 1024 bits available
    builder.append_raw(&data, 1023).unwrap();
    let cell = builder.into_cell().unwrap();

    let compressed = boc_compress_improved_structure_lz4(vec![cell.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0].bit_length(), 1023);
    assert_eq!(decompressed[0], cell);
}

/// Test roundtrip for exact byte boundary (128 bits = 16 bytes, threshold for small)
#[test]
fn test_roundtrip_small_threshold_boundary() {
    // 127 bits (small)
    let mut builder1 = BuilderData::new();
    builder1.append_raw(&[0xFFu8; 16], 127).unwrap();
    let cell1 = builder1.into_cell().unwrap();

    // 128 bits (non-small)
    let mut builder2 = BuilderData::new();
    builder2.append_raw(&[0xFFu8; 16], 128).unwrap();
    let cell2 = builder2.into_cell().unwrap();

    let compressed =
        boc_compress_improved_structure_lz4(vec![cell1.clone(), cell2.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 2);
    assert_eq!(decompressed[0], cell1);
    assert_eq!(decompressed[1], cell2);
}

/// Test empty cell (0 bits of data, no references)
#[test]
fn test_roundtrip_empty_cell() {
    let builder = BuilderData::new();
    let cell = builder.into_cell().unwrap();
    assert_eq!(cell.bit_length(), 0);

    let compressed = boc_compress_improved_structure_lz4(vec![cell.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], cell);
}

/// Test cell with exactly 1 bit of data
#[test]
fn test_roundtrip_single_bit_cell() {
    let mut builder = BuilderData::new();
    builder.append_bit_one().unwrap();
    let cell = builder.into_cell().unwrap();
    assert_eq!(cell.bit_length(), 1);

    let compressed = boc_compress_improved_structure_lz4(vec![cell.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], cell);
}

/// Test various non-byte-aligned lengths
#[test]
fn test_roundtrip_various_bit_lengths() {
    let bit_lengths =
        [1, 2, 3, 7, 8, 9, 15, 16, 17, 31, 32, 33, 63, 64, 65, 127, 128, 129, 255, 256, 257];

    for &bits in &bit_lengths {
        let mut builder = BuilderData::new();
        let byte_count = (bits + 7) / 8;
        let data = vec![0x5Au8; byte_count];
        builder.append_raw(&data, bits).unwrap();
        let cell = builder.into_cell().unwrap();

        let compressed = boc_compress_improved_structure_lz4(vec![cell.clone()]).unwrap();
        let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
        assert_eq!(decompressed.len(), 1, "Failed for bit_length={}", bits);
        assert_eq!(decompressed[0].bit_length(), bits, "Bit length mismatch for {}", bits);
        assert_eq!(decompressed[0], cell, "Cell mismatch for bit_length={}", bits);
    }
}

/// Test graph with all 4 references used
#[test]
fn test_roundtrip_max_references() {
    let c1 = build_single_cell(0x11111111);
    let c2 = build_single_cell(0x22222222);
    let c3 = build_single_cell(0x33333333);
    let c4 = build_single_cell(0x44444444);

    let mut root = BuilderData::new();
    root.append_u32(0x00000000).unwrap();
    root.checked_append_reference(c1).unwrap();
    root.checked_append_reference(c2).unwrap();
    root.checked_append_reference(c3).unwrap();
    root.checked_append_reference(c4).unwrap();
    let root_cell = root.into_cell().unwrap();

    let compressed = boc_compress_improved_structure_lz4(vec![root_cell.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], root_cell);
}

/// Test complex DAG with multiple shared references at different levels
#[test]
fn test_roundtrip_complex_dag() {
    // Build a DAG where multiple cells reference the same shared cells
    let shared_leaf = build_single_cell(0xDEAD);

    let mut mid1 = BuilderData::new();
    mid1.append_u32(1).unwrap();
    mid1.checked_append_reference(shared_leaf.clone()).unwrap();
    let mid1_cell = mid1.into_cell().unwrap();

    let mut mid2 = BuilderData::new();
    mid2.append_u32(2).unwrap();
    mid2.checked_append_reference(shared_leaf.clone()).unwrap();
    let mid2_cell = mid2.into_cell().unwrap();

    let mut mid3 = BuilderData::new();
    mid3.append_u32(3).unwrap();
    mid3.checked_append_reference(shared_leaf).unwrap();
    mid3.checked_append_reference(mid1_cell.clone()).unwrap();
    let mid3_cell = mid3.into_cell().unwrap();

    let mut root = BuilderData::new();
    root.append_u32(0).unwrap();
    root.checked_append_reference(mid1_cell).unwrap();
    root.checked_append_reference(mid2_cell).unwrap();
    root.checked_append_reference(mid3_cell).unwrap();
    let root_cell = root.into_cell().unwrap();

    let compressed = boc_compress_improved_structure_lz4(vec![root_cell.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], root_cell);
}

/// Test delta encoding threshold edge cases
#[test]
fn test_roundtrip_delta_encoding_thresholds() {
    // Build a graph with varying delta sizes to exercise all three encoding cases
    // Create a chain of 20 nodes, then add cross-references
    let mut cells = Vec::new();
    let leaf = build_single_cell(0xFFFFFFFF);
    cells.push(leaf);

    for i in 0..20 {
        let mut builder = BuilderData::new();
        builder.append_u32(i as u32).unwrap();
        builder.checked_append_reference(cells.last().unwrap().clone()).unwrap();
        cells.push(builder.into_cell().unwrap());
    }

    let root = cells.last().unwrap().clone();
    let compressed = boc_compress_improved_structure_lz4(vec![root.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], root);
}

/// Test multiple roots with shared subtrees
#[test]
fn test_roundtrip_multiple_roots_shared_subtree() {
    let shared = build_single_cell(0xCAFE);

    let mut root1 = BuilderData::new();
    root1.append_u32(1).unwrap();
    root1.checked_append_reference(shared.clone()).unwrap();
    let root1_cell = root1.into_cell().unwrap();

    let mut root2 = BuilderData::new();
    root2.append_u32(2).unwrap();
    root2.checked_append_reference(shared).unwrap();
    let root2_cell = root2.into_cell().unwrap();

    let compressed =
        boc_compress_improved_structure_lz4(vec![root1_cell.clone(), root2_cell.clone()]).unwrap();
    let decompressed = boc_decompress_improved_structure_lz4(compressed, 1024 * 1024).unwrap();
    assert_eq!(decompressed.len(), 2);
    assert_eq!(decompressed[0], root1_cell);
    assert_eq!(decompressed[1], root2_cell);
}

/// Test BaselineLZ4 round-trip for comparison
#[test]
fn test_baseline_lz4_roundtrip() {
    let cell = build_simple_tree();
    let compressed = boc_compress_baseline_lz4(vec![cell.clone()]).unwrap();
    let decompressed_bytes = boc_decompress_baseline_lz4(compressed, 1024 * 1024).unwrap();
    let decompressed =
        BocReader::new().stream_read(&mut Cursor::new(&decompressed_bytes)).unwrap().roots;
    assert_eq!(decompressed.len(), 1);
    assert_eq!(decompressed[0], cell);
}

/// Test generic boc_compress/boc_decompress API with both algorithms
#[test]
fn test_generic_api_both_algorithms() {
    let cell = build_simple_tree();

    // Test BaselineLZ4
    let compressed1 = boc_compress(vec![cell.clone()], CompressionAlgorithm::BaselineLZ4).unwrap();
    assert_eq!(compressed1[0], 0); // Algorithm byte
    let decompressed1 = boc_decompress(&compressed1, 1024 * 1024).unwrap();
    assert_eq!(decompressed1[0], cell);

    // Test ImprovedStructureLZ4
    let compressed2 =
        boc_compress(vec![cell.clone()], CompressionAlgorithm::ImprovedStructureLZ4).unwrap();
    assert_eq!(compressed2[0], 1); // Algorithm byte
    let decompressed2 = boc_decompress(&compressed2, 1024 * 1024).unwrap();
    assert_eq!(decompressed2[0], cell);
}

// == [ Some helpers to build cells ] ==

use crate::BocReader;
use std::io::Cursor;

/// Helper function to build a simple cell tree for testing
/// Structure:
///     root
///   /  |  \
///  c1  c2  c3
///  |
///  c4
fn build_simple_tree() -> Cell {
    let mut c4 = BuilderData::new();
    c4.append_u32(4).unwrap();
    let c4_cell = c4.into_cell().unwrap();

    let mut c1 = BuilderData::new();
    c1.append_u32(1).unwrap();
    c1.checked_append_reference(c4_cell).unwrap();
    let c1_cell = c1.into_cell().unwrap();

    let mut c2 = BuilderData::new();
    c2.append_u32(2).unwrap();
    let c2_cell = c2.into_cell().unwrap();

    let mut c3 = BuilderData::new();
    c3.append_u32(3).unwrap();
    let c3_cell = c3.into_cell().unwrap();

    let mut root = BuilderData::new();
    root.append_u32(0).unwrap();
    root.checked_append_reference(c1_cell).unwrap();
    root.checked_append_reference(c2_cell).unwrap();
    root.checked_append_reference(c3_cell).unwrap();

    root.into_cell().unwrap()
}

/// Helper function to build a single cell with data
fn build_single_cell(data: u64) -> Cell {
    let mut builder = BuilderData::new();
    builder.append_u64(data).unwrap();
    builder.into_cell().unwrap()
}

/// Helper function to build a cell tree with varying data sizes
fn build_tree_with_large_data() -> Cell {
    let mut leaf = BuilderData::new();
    // Fill with data close to max (1023 bits = ~127 bytes, use 120 bytes = 30 u32s)
    for i in 0..30u32 {
        leaf.append_u32(i).unwrap();
    }
    let leaf_cell = leaf.into_cell().unwrap();

    let mut root = BuilderData::new();
    root.append_u64(0xDEADBEEF_CAFEBABE).unwrap();
    root.checked_append_reference(leaf_cell).unwrap();

    root.into_cell().unwrap()
}

/// Helper function to build a deep tree (chain of cells)
fn build_deep_tree(depth: usize) -> Cell {
    let mut current: Option<Cell> = None;

    for i in 0..depth {
        let mut builder = BuilderData::new();
        builder.append_u32(i as u32).unwrap();
        if let Some(child) = current {
            builder.checked_append_reference(child).unwrap();
        }
        current = Some(builder.into_cell().unwrap());
    }

    current.unwrap()
}

/// Helper function to build a tree with shared references (DAG structure)
fn build_dag_tree() -> Cell {
    // Create a shared cell
    let mut shared = BuilderData::new();
    shared.append_u32(0x5AAED).unwrap();
    let shared_cell = shared.into_cell().unwrap();

    // Create two cells that reference the same shared cell
    let mut c1 = BuilderData::new();
    c1.append_u32(1).unwrap();
    c1.checked_append_reference(shared_cell.clone()).unwrap();
    let c1_cell = c1.into_cell().unwrap();

    let mut c2 = BuilderData::new();
    c2.append_u32(2).unwrap();
    c2.checked_append_reference(shared_cell).unwrap();
    let c2_cell = c2.into_cell().unwrap();

    // Root references both
    let mut root = BuilderData::new();
    root.append_u32(0).unwrap();
    root.checked_append_reference(c1_cell).unwrap();
    root.checked_append_reference(c2_cell).unwrap();

    root.into_cell().unwrap()
}

// ============================================
// C++ Compatibility Test Vectors
// ============================================
//
// These test vectors are real-world BOC files compressed by the C++ implementation.
// They serve as regression tests to ensure wire compatibility between Rust and C++.
//
// Test files:
// - cpp_compat_simple_1.boc, cpp_compat_simple_2.boc: Simple BOCs without depth-balance elision
// - cpp_compat_depth_balance_*.boc: BOCs with MerkleUpdate cells that use depth-balance
//   elision optimization (cell_type=9). These files previously caused recompression
//   mismatches before the depth-balance elision was implemented in Rust.

/// Test C++ compatibility: simple BOC without depth-balance elision
#[test]
fn test_cpp_compat_simple_1() {
    // This BOC was compressed by C++ and should decompress + recompress to identical bytes
    const COMPRESSED: &[u8] = include_bytes!("test_data/cpp_compat_simple_1.boc");

    // Skip algorithm prefix byte (0x01 = ImprovedStructureLZ4)
    assert_eq!(COMPRESSED[0], 0x01, "Expected ImprovedStructureLZ4 algorithm prefix");
    let compressed_payload = &COMPRESSED[1..];

    let decompressed =
        boc_decompress_improved_structure_lz4(compressed_payload.to_vec(), 1024 * 1024)
            .expect("Failed to decompress C++ BOC");

    let recompressed =
        boc_compress_improved_structure_lz4(decompressed).expect("Failed to recompress BOC");

    assert_eq!(
        recompressed, compressed_payload,
        "Recompressed BOC doesn't match C++ original (cpp_compat_simple_1)"
    );
}

/// Test C++ compatibility: simple BOC without depth-balance elision (variant 2)
#[test]
fn test_cpp_compat_simple_2() {
    const COMPRESSED: &[u8] = include_bytes!("test_data/cpp_compat_simple_2.boc");

    assert_eq!(COMPRESSED[0], 0x01, "Expected ImprovedStructureLZ4 algorithm prefix");
    let compressed_payload = &COMPRESSED[1..];

    let decompressed =
        boc_decompress_improved_structure_lz4(compressed_payload.to_vec(), 1024 * 1024)
            .expect("Failed to decompress C++ BOC");

    let recompressed =
        boc_compress_improved_structure_lz4(decompressed).expect("Failed to recompress BOC");

    assert_eq!(
        recompressed, compressed_payload,
        "Recompressed BOC doesn't match C++ original (cpp_compat_simple_2)"
    );
}

/// Test C++ compatibility: BOC with MerkleUpdate depth-balance elision (cell_type=9)
///
/// This test exercises the depth-balance elision optimization where cells in the right
/// subtree of a MerkleUpdate can have their data omitted if it can be reconstructed
/// from the paired left subtree cell and children differences.
#[test]
fn test_cpp_compat_depth_balance_1() {
    const COMPRESSED: &[u8] = include_bytes!("test_data/cpp_compat_depth_balance_1.boc");

    assert_eq!(COMPRESSED[0], 0x01, "Expected ImprovedStructureLZ4 algorithm prefix");
    let compressed_payload = &COMPRESSED[1..];

    let decompressed =
        boc_decompress_improved_structure_lz4(compressed_payload.to_vec(), 1024 * 1024)
            .expect("Failed to decompress C++ BOC with depth-balance elision");

    let recompressed =
        boc_compress_improved_structure_lz4(decompressed).expect("Failed to recompress BOC");

    assert_eq!(
        recompressed, compressed_payload,
        "Recompressed BOC doesn't match C++ original (cpp_compat_depth_balance_1). \
         This indicates a problem with depth-balance elision encoding."
    );
}

/// Test C++ compatibility: BOC with MerkleUpdate depth-balance elision (variant 2)
#[test]
fn test_cpp_compat_depth_balance_2() {
    const COMPRESSED: &[u8] = include_bytes!("test_data/cpp_compat_depth_balance_2.boc");

    assert_eq!(COMPRESSED[0], 0x01, "Expected ImprovedStructureLZ4 algorithm prefix");
    let compressed_payload = &COMPRESSED[1..];

    let decompressed =
        boc_decompress_improved_structure_lz4(compressed_payload.to_vec(), 1024 * 1024)
            .expect("Failed to decompress C++ BOC with depth-balance elision");

    let recompressed =
        boc_compress_improved_structure_lz4(decompressed).expect("Failed to recompress BOC");

    assert_eq!(
        recompressed, compressed_payload,
        "Recompressed BOC doesn't match C++ original (cpp_compat_depth_balance_2). \
         This indicates a problem with depth-balance elision encoding."
    );
}

/// Test C++ compatibility: BOC with MerkleUpdate depth-balance elision (variant 3)
#[test]
fn test_cpp_compat_depth_balance_3() {
    const COMPRESSED: &[u8] = include_bytes!("test_data/cpp_compat_depth_balance_3.boc");

    assert_eq!(COMPRESSED[0], 0x01, "Expected ImprovedStructureLZ4 algorithm prefix");
    let compressed_payload = &COMPRESSED[1..];

    let decompressed =
        boc_decompress_improved_structure_lz4(compressed_payload.to_vec(), 1024 * 1024)
            .expect("Failed to decompress C++ BOC with depth-balance elision");

    let recompressed =
        boc_compress_improved_structure_lz4(decompressed).expect("Failed to recompress BOC");

    assert_eq!(
        recompressed, compressed_payload,
        "Recompressed BOC doesn't match C++ original (cpp_compat_depth_balance_3). \
         This indicates a problem with depth-balance elision encoding."
    );
}

/// Test that all C++ compatibility BOCs can be decompressed via the generic API
#[test]
fn test_cpp_compat_generic_api() {
    let test_files: &[&[u8]] = &[
        include_bytes!("test_data/cpp_compat_simple_1.boc"),
        include_bytes!("test_data/cpp_compat_simple_2.boc"),
        include_bytes!("test_data/cpp_compat_depth_balance_1.boc"),
        include_bytes!("test_data/cpp_compat_depth_balance_2.boc"),
        include_bytes!("test_data/cpp_compat_depth_balance_3.boc"),
    ];

    for (i, compressed) in test_files.iter().enumerate() {
        let decompressed = boc_decompress(compressed, 1024 * 1024)
            .unwrap_or_else(|e| panic!("Failed to decompress test file {}: {}", i, e));

        assert!(!decompressed.is_empty(), "Test file {} produced no roots", i);

        // Verify we can recompress with the generic API
        let recompressed = boc_compress(decompressed, CompressionAlgorithm::ImprovedStructureLZ4)
            .unwrap_or_else(|e| panic!("Failed to recompress test file {}: {}", i, e));

        assert_eq!(recompressed.as_slice(), *compressed, "Test file {} recompression mismatch", i);
    }
}
