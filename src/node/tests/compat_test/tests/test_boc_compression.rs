/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! BOC Compression cross-implementation tests.
//!
//! Tests that BOC compressed by one implementation (Rust or C++) can be
//! decompressed by the other, verifying wire-format compatibility.
//!
//! The compress/decompress commands are standalone (no networking required),
//! so each test only spawns a single CppTestNode as a compression oracle.

use compat_test::{skip_if_no_cpp, CppTestNode};
use ton_block::{
    boc_compression::{boc_compress, boc_decompress, CompressionAlgorithm},
    read_boc, write_boc, write_boc_multi, BuilderData, Cell, IBitstring,
};

const PORT_BASE: u16 = 15500;
const MAX_DECOMPRESS_SIZE: u32 = 10 * 1024 * 1024;

// ---- Cell construction helpers ----

fn build_single_cell(data: u64) -> Cell {
    let mut builder = BuilderData::new();
    builder.append_u64(data).unwrap();
    builder.into_cell().unwrap()
}

fn build_simple_tree() -> Cell {
    let mut leaf = BuilderData::new();
    leaf.append_u64(0xDEADBEEF_CAFEBABE).unwrap();
    let leaf_cell = leaf.into_cell().unwrap();

    let mut child1 = BuilderData::new();
    child1.append_u32(1).unwrap();
    child1.checked_append_reference(leaf_cell).unwrap();
    let child1_cell = child1.into_cell().unwrap();

    let mut child2 = BuilderData::new();
    child2.append_u32(2).unwrap();
    let child2_cell = child2.into_cell().unwrap();

    let mut root = BuilderData::new();
    root.append_u32(0).unwrap();
    root.checked_append_reference(child1_cell).unwrap();
    root.checked_append_reference(child2_cell).unwrap();
    root.into_cell().unwrap()
}

fn build_dag_tree() -> Cell {
    let mut shared = BuilderData::new();
    shared.append_u32(0x5AAED).unwrap();
    let shared_cell = shared.into_cell().unwrap();

    let mut c1 = BuilderData::new();
    c1.append_u32(1).unwrap();
    c1.checked_append_reference(shared_cell.clone()).unwrap();
    let c1_cell = c1.into_cell().unwrap();

    let mut c2 = BuilderData::new();
    c2.append_u32(2).unwrap();
    c2.checked_append_reference(shared_cell).unwrap();
    let c2_cell = c2.into_cell().unwrap();

    let mut root = BuilderData::new();
    root.append_u32(0).unwrap();
    root.checked_append_reference(c1_cell).unwrap();
    root.checked_append_reference(c2_cell).unwrap();
    root.into_cell().unwrap()
}

fn cells_to_boc_b64(cells: &[Cell]) -> String {
    let boc_bytes = if cells.len() == 1 {
        write_boc(&cells[0]).unwrap()
    } else {
        write_boc_multi(cells.to_vec()).unwrap()
    };
    base64::engine::general_purpose::STANDARD.encode(&boc_bytes)
}

fn b64_to_cells(b64: &str) -> Vec<Cell> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
    let result = read_boc(&bytes).unwrap();
    result.roots
}

use base64::Engine;

// ---- Tests ----

/// Rust compresses BOC, C++ decompresses it — verify cell hashes match.
#[test]
fn test_boc_compress_rust_decompress_cpp() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(PORT_BASE).expect("spawn C++");

    let test_cases: Vec<(&str, Vec<Cell>)> = vec![
        ("single_cell", vec![build_single_cell(0x12345678)]),
        ("simple_tree", vec![build_simple_tree()]),
        ("dag_tree", vec![build_dag_tree()]),
    ];

    for algo_name in &["baseline", "improved"] {
        let rust_algo = match *algo_name {
            "baseline" => CompressionAlgorithm::BaselineLZ4,
            "improved" => CompressionAlgorithm::ImprovedStructureLZ4,
            _ => unreachable!(),
        };

        for (name, cells) in &test_cases {
            println!("Testing Rust compress -> C++ decompress: {} ({})", name, algo_name);

            // Rust compresses
            let compressed = boc_compress(cells.clone(), rust_algo).unwrap();

            // Send compressed to C++ for decompression
            let compressed_b64 = base64::engine::general_purpose::STANDARD.encode(&compressed);
            let decompressed_boc_b64 =
                cpp.decompress_boc(&compressed_b64, MAX_DECOMPRESS_SIZE).expect("C++ decompress");

            // Parse the decompressed BOC back into cells
            let decompressed_cells = b64_to_cells(&decompressed_boc_b64);

            // Verify cell hashes match
            assert_eq!(
                cells.len(),
                decompressed_cells.len(),
                "{} ({}): root count mismatch",
                name,
                algo_name
            );
            for (i, (original, decompressed)) in
                cells.iter().zip(decompressed_cells.iter()).enumerate()
            {
                assert_eq!(
                    original.repr_hash(),
                    decompressed.repr_hash(),
                    "{} ({}): root {} hash mismatch",
                    name,
                    algo_name,
                    i
                );
            }
            println!("  OK: {} roots verified", cells.len());
        }
    }

    cpp.shutdown().expect("shutdown");
}

/// C++ compresses BOC, Rust decompresses it — verify cell hashes match.
#[test]
fn test_boc_compress_cpp_decompress_rust() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(PORT_BASE + 10).expect("spawn C++");

    let test_cases: Vec<(&str, Vec<Cell>)> = vec![
        ("single_cell", vec![build_single_cell(0x12345678)]),
        ("simple_tree", vec![build_simple_tree()]),
        ("dag_tree", vec![build_dag_tree()]),
    ];

    for algo_name in &["baseline", "improved"] {
        for (name, cells) in &test_cases {
            println!("Testing C++ compress -> Rust decompress: {} ({})", name, algo_name);

            // Send standard BOC to C++ for compression
            let boc_b64 = cells_to_boc_b64(cells);
            let compressed_b64 = cpp.compress_boc(&boc_b64, algo_name).expect("C++ compress");

            // Rust decompresses
            let compressed_bytes =
                base64::engine::general_purpose::STANDARD.decode(&compressed_b64).unwrap();
            let decompressed_cells =
                boc_decompress(&compressed_bytes, MAX_DECOMPRESS_SIZE as usize).unwrap();

            // Verify cell hashes match
            assert_eq!(
                cells.len(),
                decompressed_cells.len(),
                "{} ({}): root count mismatch",
                name,
                algo_name
            );
            for (i, (original, decompressed)) in
                cells.iter().zip(decompressed_cells.iter()).enumerate()
            {
                assert_eq!(
                    original.repr_hash(),
                    decompressed.repr_hash(),
                    "{} ({}): root {} hash mismatch",
                    name,
                    algo_name,
                    i
                );
            }
            println!("  OK: {} roots verified", cells.len());
        }
    }

    cpp.shutdown().expect("shutdown");
}

/// Full round-trip: Rust compress -> C++ decompress -> C++ compress -> Rust decompress.
/// Verifies that data survives two cross-implementation transitions.
#[test]
fn test_boc_compression_roundtrip() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(PORT_BASE + 20).expect("spawn C++");

    let test_cases: Vec<(&str, Vec<Cell>)> = vec![
        ("single_cell", vec![build_single_cell(0xAAAABBBB)]),
        ("simple_tree", vec![build_simple_tree()]),
        ("dag_tree", vec![build_dag_tree()]),
    ];

    for algo_name in &["baseline", "improved"] {
        let rust_algo = match *algo_name {
            "baseline" => CompressionAlgorithm::BaselineLZ4,
            "improved" => CompressionAlgorithm::ImprovedStructureLZ4,
            _ => unreachable!(),
        };

        for (name, cells) in &test_cases {
            println!("Testing full round-trip: {} ({})", name, algo_name);

            // Step 1: Rust compresses
            let compressed1 = boc_compress(cells.clone(), rust_algo).unwrap();
            let compressed1_b64 = base64::engine::general_purpose::STANDARD.encode(&compressed1);

            // Step 2: C++ decompresses
            let decompressed_boc_b64 =
                cpp.decompress_boc(&compressed1_b64, MAX_DECOMPRESS_SIZE).expect("C++ decompress");

            // Step 3: C++ compresses again
            let compressed2_b64 =
                cpp.compress_boc(&decompressed_boc_b64, algo_name).expect("C++ compress");

            // Step 4: Rust decompresses
            let compressed2_bytes =
                base64::engine::general_purpose::STANDARD.decode(&compressed2_b64).unwrap();
            let final_cells =
                boc_decompress(&compressed2_bytes, MAX_DECOMPRESS_SIZE as usize).unwrap();

            // Verify cell hashes match the original
            assert_eq!(
                cells.len(),
                final_cells.len(),
                "{} ({}): root count mismatch after round-trip",
                name,
                algo_name
            );
            for (i, (original, final_cell)) in cells.iter().zip(final_cells.iter()).enumerate() {
                assert_eq!(
                    original.repr_hash(),
                    final_cell.repr_hash(),
                    "{} ({}): root {} hash mismatch after round-trip",
                    name,
                    algo_name,
                    i
                );
            }
            println!("  OK: full round-trip verified");
        }
    }

    cpp.shutdown().expect("shutdown");
}

/// Test with multiple root cells (multi-root BOC).
#[test]
fn test_boc_compression_multi_root() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(PORT_BASE + 30).expect("spawn C++");

    let roots = vec![build_single_cell(1), build_single_cell(2), build_simple_tree()];

    for algo_name in &["baseline", "improved"] {
        let rust_algo = match *algo_name {
            "baseline" => CompressionAlgorithm::BaselineLZ4,
            "improved" => CompressionAlgorithm::ImprovedStructureLZ4,
            _ => unreachable!(),
        };

        println!("Testing multi-root BOC ({})", algo_name);

        // Rust compress -> C++ decompress
        let compressed = boc_compress(roots.clone(), rust_algo).unwrap();
        let compressed_b64 = base64::engine::general_purpose::STANDARD.encode(&compressed);
        let decompressed_boc_b64 = cpp
            .decompress_boc(&compressed_b64, MAX_DECOMPRESS_SIZE)
            .expect("C++ decompress multi-root");
        let decompressed = b64_to_cells(&decompressed_boc_b64);

        assert_eq!(roots.len(), decompressed.len(), "multi-root ({}): count mismatch", algo_name);
        for (i, (orig, dec)) in roots.iter().zip(decompressed.iter()).enumerate() {
            assert_eq!(
                orig.repr_hash(),
                dec.repr_hash(),
                "multi-root ({}): root {} hash mismatch",
                algo_name,
                i
            );
        }

        // C++ compress -> Rust decompress
        let boc_b64 = cells_to_boc_b64(&roots);
        let cpp_compressed_b64 =
            cpp.compress_boc(&boc_b64, algo_name).expect("C++ compress multi-root");
        let cpp_compressed =
            base64::engine::general_purpose::STANDARD.decode(&cpp_compressed_b64).unwrap();
        let cpp_decompressed =
            boc_decompress(&cpp_compressed, MAX_DECOMPRESS_SIZE as usize).unwrap();

        assert_eq!(
            roots.len(),
            cpp_decompressed.len(),
            "multi-root ({}): count mismatch (C++ direction)",
            algo_name
        );
        for (i, (orig, dec)) in roots.iter().zip(cpp_decompressed.iter()).enumerate() {
            assert_eq!(
                orig.repr_hash(),
                dec.repr_hash(),
                "multi-root ({}): root {} hash mismatch (C++ direction)",
                algo_name,
                i
            );
        }
        println!("  OK: {} roots verified both directions", roots.len());
    }

    cpp.shutdown().expect("shutdown");
}
