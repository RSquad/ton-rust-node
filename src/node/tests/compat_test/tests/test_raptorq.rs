/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! RaptorQ FEC cross-implementation compatibility tests.
//!
//! Tests that symbols encoded by one implementation (Rust or C++) can be
//! decoded by the other, ensuring the RaptorQ codec is wire-compatible.

use adnl::{RaptorqDecoder, RaptorqEncoder};
use compat_test::{skip_if_no_cpp, CppTestNode, EncodedSymbol, TestTimeout};
use ton_api::ton::fec::type_::RaptorQ as FecTypeRaptorQ;
use ton_block::sha256_digest;

/// Port range 19000-19099 for raptorq tests
const BASE_PORT: u16 = 19000;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn rust_encode(
    data: &[u8],
    symbol_size: Option<u32>,
    repair_count: u32,
) -> (FecTypeRaptorQ, Vec<EncodedSymbol>) {
    let mut encoder = RaptorqEncoder::with_data(data, symbol_size);
    let params = encoder.params().clone();
    let source_count = params.symbols_count as u32;
    let total = source_count + repair_count;

    let mut symbols = Vec::new();
    let mut seqno = 0u32;
    for _ in 0..total {
        let prev = seqno;
        let chunk = encoder.encode(&mut seqno).expect("encode failed");
        symbols.push(EncodedSymbol {
            id: seqno,
            data: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &chunk),
        });
        if prev == seqno {
            seqno += 1;
        }
    }

    (params, symbols)
}

fn rust_decode(params: &FecTypeRaptorQ, symbols: &[EncodedSymbol]) -> Vec<u8> {
    let mut decoder = RaptorqDecoder::with_params(params.clone()).expect("decoder creation failed");
    for sym in symbols {
        let data = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &sym.data)
            .expect("bad base64");
        if let Some(result) = decoder.decode(sym.id, &data) {
            return result;
        }
    }
    panic!(
        "Rust decoder failed to reconstruct (fed {} symbols, needed {})",
        symbols.len(),
        params.symbols_count
    );
}

fn make_test_data(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 251) as u8).collect()
}

// ---------------------------------------------------------------------------
// Tests: Rust encode -> C++ decode
// ---------------------------------------------------------------------------

#[test]
fn test_rust_encode_cpp_decode_small() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT).expect("spawn C++ node");

    let data = make_test_data(500);
    let (params, symbols) = rust_encode(&data, None, 0);

    println!(
        "Rust encoded: data_size={}, symbol_size={}, symbols_count={}, generated={}",
        params.data_size,
        params.symbol_size,
        params.symbols_count,
        symbols.len()
    );

    let decoded = cpp
        .raptorq_decode(
            params.data_size as u32,
            params.symbol_size as u32,
            params.symbols_count as u32,
            &symbols,
        )
        .expect("C++ decode failed");

    assert_eq!(decoded.len(), data.len(), "decoded size mismatch");
    assert_eq!(decoded, data, "decoded data mismatch");
    println!("OK: Rust encode -> C++ decode ({}B, source-only)", data.len());
}

#[test]
fn test_rust_encode_cpp_decode_medium() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 1).expect("spawn C++ node");

    // ~10 KB: will produce multiple source symbols with 768-byte symbol_size
    let data = make_test_data(10_000);
    let (params, symbols) = rust_encode(&data, None, 2);

    println!(
        "Rust encoded: data_size={}, symbol_size={}, symbols_count={}, generated={}",
        params.data_size,
        params.symbol_size,
        params.symbols_count,
        symbols.len()
    );

    let decoded = cpp
        .raptorq_decode(
            params.data_size as u32,
            params.symbol_size as u32,
            params.symbols_count as u32,
            &symbols,
        )
        .expect("C++ decode failed");

    assert_eq!(decoded.len(), data.len(), "decoded size mismatch");
    assert_eq!(decoded, data, "decoded data mismatch");
    println!("OK: Rust encode -> C++ decode ({}B, with repair)", data.len());
}

#[test]
fn test_rust_encode_cpp_decode_large() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(60);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 2).expect("spawn C++ node");

    // ~100 KB
    let data = make_test_data(100_000);
    let (params, symbols) = rust_encode(&data, None, 4);

    println!(
        "Rust encoded: data_size={}, symbol_size={}, symbols_count={}, generated={}",
        params.data_size,
        params.symbol_size,
        params.symbols_count,
        symbols.len()
    );

    let decoded = cpp
        .raptorq_decode(
            params.data_size as u32,
            params.symbol_size as u32,
            params.symbols_count as u32,
            &symbols,
        )
        .expect("C++ decode failed");

    assert_eq!(decoded.len(), data.len(), "decoded size mismatch");
    assert_eq!(decoded, data, "decoded data mismatch");
    println!("OK: Rust encode -> C++ decode ({}B, large)", data.len());
}

// ---------------------------------------------------------------------------
// Tests: C++ encode -> Rust decode
// ---------------------------------------------------------------------------

#[test]
fn test_cpp_encode_rust_decode_small() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 10).expect("spawn C++ node");

    let data = make_test_data(500);
    let result = cpp.raptorq_encode(&data, 768, 0).expect("C++ encode failed");

    println!(
        "C++ encoded: data_size={}, symbol_size={}, symbols_count={}, generated={}",
        result.data_size,
        result.symbol_size,
        result.symbols_count,
        result.symbols.len()
    );

    let params = FecTypeRaptorQ {
        data_size: result.data_size as i32,
        symbol_size: result.symbol_size as i32,
        symbols_count: result.symbols_count as i32,
    };

    let decoded = rust_decode(&params, &result.symbols);
    assert_eq!(decoded.len(), data.len(), "decoded size mismatch");
    assert_eq!(decoded, data, "decoded data mismatch");
    println!("OK: C++ encode -> Rust decode ({}B, source-only)", data.len());
}

#[test]
fn test_cpp_encode_rust_decode_medium() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 11).expect("spawn C++ node");

    let data = make_test_data(10_000);
    let result = cpp.raptorq_encode(&data, 768, 2).expect("C++ encode failed");

    println!(
        "C++ encoded: data_size={}, symbol_size={}, symbols_count={}, generated={}",
        result.data_size,
        result.symbol_size,
        result.symbols_count,
        result.symbols.len()
    );

    let params = FecTypeRaptorQ {
        data_size: result.data_size as i32,
        symbol_size: result.symbol_size as i32,
        symbols_count: result.symbols_count as i32,
    };

    let decoded = rust_decode(&params, &result.symbols);
    assert_eq!(decoded.len(), data.len(), "decoded size mismatch");
    assert_eq!(decoded, data, "decoded data mismatch");
    println!("OK: C++ encode -> Rust decode ({}B, with repair)", data.len());
}

#[test]
fn test_cpp_encode_rust_decode_large() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(60);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 12).expect("spawn C++ node");

    let data = make_test_data(100_000);
    let result = cpp.raptorq_encode(&data, 768, 4).expect("C++ encode failed");

    println!(
        "C++ encoded: data_size={}, symbol_size={}, symbols_count={}, generated={}",
        result.data_size,
        result.symbol_size,
        result.symbols_count,
        result.symbols.len()
    );

    let params = FecTypeRaptorQ {
        data_size: result.data_size as i32,
        symbol_size: result.symbol_size as i32,
        symbols_count: result.symbols_count as i32,
    };

    let decoded = rust_decode(&params, &result.symbols);
    assert_eq!(decoded.len(), data.len(), "decoded size mismatch");
    assert_eq!(decoded, data, "decoded data mismatch");
    println!("OK: C++ encode -> Rust decode ({}B, large)", data.len());
}

// ---------------------------------------------------------------------------
// Tests: repair-only decode (drop some source, keep repair)
// ---------------------------------------------------------------------------

#[test]
fn test_rust_encode_cpp_decode_with_loss() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 20).expect("spawn C++ node");

    let data = make_test_data(10_000);
    let source_count_extra = 4u32; // extra repair symbols
    let (params, all_symbols) = rust_encode(&data, None, source_count_extra);
    let source_count = params.symbols_count as usize;

    // Drop first 2 source symbols, keep the rest + all repair
    let symbols: Vec<_> = all_symbols[2..].to_vec();
    println!(
        "Rust encoded {} symbols ({}+{} repair), feeding {} (dropped 2 source) to C++",
        all_symbols.len(),
        source_count,
        source_count_extra,
        symbols.len()
    );

    let decoded = cpp
        .raptorq_decode(
            params.data_size as u32,
            params.symbol_size as u32,
            params.symbols_count as u32,
            &symbols,
        )
        .expect("C++ decode failed with loss");

    assert_eq!(decoded, data, "decoded data mismatch after loss");
    println!("OK: Rust encode -> C++ decode with simulated loss ({}B)", data.len());
}

#[test]
fn test_cpp_encode_rust_decode_with_loss() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 21).expect("spawn C++ node");

    let data = make_test_data(10_000);
    let result = cpp.raptorq_encode(&data, 768, 4).expect("C++ encode failed");
    let source_count = result.symbols_count as usize;

    // Drop first 2 source symbols, keep the rest + repair
    let symbols: Vec<_> = result.symbols[2..].to_vec();
    println!(
        "C++ encoded {} symbols ({}+4 repair), feeding {} (dropped 2 source) to Rust",
        result.symbols.len(),
        source_count,
        symbols.len()
    );

    let params = FecTypeRaptorQ {
        data_size: result.data_size as i32,
        symbol_size: result.symbol_size as i32,
        symbols_count: result.symbols_count as i32,
    };

    let decoded = rust_decode(&params, &symbols);
    assert_eq!(decoded, data, "decoded data mismatch after loss");
    println!("OK: C++ encode -> Rust decode with simulated loss ({}B)", data.len());
}

// ---------------------------------------------------------------------------
// Tests: parameter agreement
// ---------------------------------------------------------------------------

#[test]
fn test_params_match() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 30).expect("spawn C++ node");

    // Verify both implementations agree on encoding parameters for various data sizes
    for &data_size in &[100, 500, 768, 769, 1536, 5000, 10000, 50000, 100000] {
        let data = make_test_data(data_size);

        let rust_encoder = RaptorqEncoder::with_data(&data, None);
        let rust_params = rust_encoder.params();

        let cpp_result = cpp.raptorq_encode(&data, 768, 0).expect("C++ encode failed");

        assert_eq!(
            rust_params.data_size as u32, cpp_result.data_size,
            "data_size mismatch for input size {}",
            data_size
        );
        assert_eq!(
            rust_params.symbol_size as u32, cpp_result.symbol_size,
            "symbol_size mismatch for input size {}",
            data_size
        );
        assert_eq!(
            rust_params.symbols_count as u32, cpp_result.symbols_count,
            "symbols_count mismatch for input size {}",
            data_size
        );

        println!(
            "params match for data_size={}: symbols_count={}, symbol_size={}",
            data_size, cpp_result.symbols_count, cpp_result.symbol_size
        );
    }
    println!("OK: all parameter sets match between Rust and C++");
}

// ---------------------------------------------------------------------------
// Tests: source symbols are byte-identical
// ---------------------------------------------------------------------------

#[test]
fn test_source_symbols_identical() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(30);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 31).expect("spawn C++ node");

    for &data_size in &[500, 5000, 50000] {
        let data = make_test_data(data_size);

        let (rust_params, rust_symbols) = rust_encode(&data, None, 0);
        let cpp_result = cpp.raptorq_encode(&data, 768, 0).expect("C++ encode failed");

        let source_count = rust_params.symbols_count as usize;
        assert_eq!(source_count, cpp_result.symbols_count as usize);

        for i in 0..source_count {
            let rust_sym_data = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &rust_symbols[i].data,
            )
            .expect("bad base64");
            let cpp_sym_data = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &cpp_result.symbols[i].data,
            )
            .expect("bad base64");

            assert_eq!(
                rust_sym_data, cpp_sym_data,
                "source symbol {} differs for data_size={}",
                i, data_size
            );
        }
        println!("OK: {} source symbols identical for data_size={}", source_count, data_size);
    }
}

// ---------------------------------------------------------------------------
// Tests: 4MB large-symbol bidirectional FEC (symbol sizes >= 64 KB)
// ---------------------------------------------------------------------------

/// 4 MB — maximum block size used in TON.
const DATA_4MB: usize = 4 * 1024 * 1024;

/// Symbol counts to test.  All yield symbol_size >= 64 KB for 4 MB data:
///   4 → 1 MB, 8 → 512 KB, 16 → 256 KB, 32 → 128 KB, 64 → 64 KB.
const SYMBOL_COUNTS: &[u32] = &[4, 8, 16, 32, 64];

fn make_random_data(size: usize) -> Vec<u8> {
    // LCG to generate deterministic pseudo-random data (fast, no extra deps)
    let mut v = vec![0u8; size];
    let mut state: u64 = 0xDEAD_BEEF_CAFE_BABE;
    for chunk in v.chunks_mut(8) {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let bytes = state.to_le_bytes();
        let len = chunk.len();
        chunk.copy_from_slice(&bytes[..len]);
    }
    v
}

fn hash_hex(data: &[u8]) -> String {
    hex::encode(sha256_digest(data))
}

/// symbol_size = ceil(data_len / symbol_count)
fn symbol_size_for(data_len: usize, symbol_count: u32) -> u32 {
    ((data_len + symbol_count as usize - 1) / symbol_count as usize) as u32
}

#[test]
fn test_4mb_rust_encode_cpp_decode() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(300);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 40).expect("spawn C++ node");

    let data = make_random_data(DATA_4MB);
    let original_hash = hash_hex(&data);
    println!("original SHA-256: {}", original_hash);

    for &sym_count in SYMBOL_COUNTS {
        let sym_size = symbol_size_for(data.len(), sym_count);
        println!(
            "\n--- Rust encode -> C++ decode: {} symbols, symbol_size={} ({} KB) ---",
            sym_count,
            sym_size,
            sym_size / 1024,
        );

        let (params, symbols) = rust_encode(&data, Some(sym_size), 0);
        assert_eq!(
            params.symbols_count, sym_count as i32,
            "unexpected symbols_count for sym_size={}",
            sym_size,
        );
        println!(
            "Rust encoded: data_size={}, symbol_size={}, symbols_count={}",
            params.data_size, params.symbol_size, params.symbols_count,
        );

        let decoded = cpp
            .raptorq_decode(
                params.data_size as u32,
                params.symbol_size as u32,
                params.symbols_count as u32,
                &symbols,
            )
            .expect("C++ decode failed");

        let decoded_hash = hash_hex(&decoded);
        println!("decoded  SHA-256: {}", decoded_hash);
        assert_eq!(
            original_hash, decoded_hash,
            "SHA-256 mismatch for {} symbols (sym_size={}): Rust encode -> C++ decode",
            sym_count, sym_size,
        );
        println!("OK: hash match ({} symbols)", sym_count);
    }
}

#[test]
fn test_4mb_cpp_encode_rust_decode() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(300);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 41).expect("spawn C++ node");

    let data = make_random_data(DATA_4MB);
    let original_hash = hash_hex(&data);
    println!("original SHA-256: {}", original_hash);

    for &sym_count in SYMBOL_COUNTS {
        let sym_size = symbol_size_for(data.len(), sym_count);
        println!(
            "\n--- C++ encode -> Rust decode: {} symbols, symbol_size={} ({} KB) ---",
            sym_count,
            sym_size,
            sym_size / 1024,
        );

        let result = cpp.raptorq_encode(&data, sym_size, 0).expect("C++ encode failed");
        assert_eq!(
            result.symbols_count, sym_count,
            "unexpected symbols_count for sym_size={}",
            sym_size,
        );
        println!(
            "C++ encoded: data_size={}, symbol_size={}, symbols_count={}",
            result.data_size, result.symbol_size, result.symbols_count,
        );

        let params = FecTypeRaptorQ {
            data_size: result.data_size as i32,
            symbol_size: result.symbol_size as i32,
            symbols_count: result.symbols_count as i32,
        };

        let decoded = rust_decode(&params, &result.symbols);
        let decoded_hash = hash_hex(&decoded);
        println!("decoded  SHA-256: {}", decoded_hash);
        assert_eq!(
            original_hash, decoded_hash,
            "SHA-256 mismatch for {} symbols (sym_size={}): C++ encode -> Rust decode",
            sym_count, sym_size,
        );
        println!("OK: hash match ({} symbols)", sym_count);
    }
}

#[test]
fn test_4mb_large_symbol_params_match() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(300);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 42).expect("spawn C++ node");

    let data = make_random_data(DATA_4MB);

    for &sym_count in SYMBOL_COUNTS {
        let sym_size = symbol_size_for(data.len(), sym_count);

        let rust_encoder = RaptorqEncoder::with_data(&data, Some(sym_size));
        let rp = rust_encoder.params();

        let cpp_result = cpp.raptorq_encode(&data, sym_size, 0).expect("C++ encode failed");

        assert_eq!(
            rp.data_size as u32, cpp_result.data_size,
            "data_size mismatch for {} symbols",
            sym_count,
        );
        assert_eq!(
            rp.symbol_size as u32, cpp_result.symbol_size,
            "symbol_size mismatch for {} symbols",
            sym_count,
        );
        assert_eq!(
            rp.symbols_count as u32, cpp_result.symbols_count,
            "symbols_count mismatch for {} symbols",
            sym_count,
        );
        println!(
            "OK: params match for {} symbols: symbol_size={}, symbols_count={}",
            sym_count, cpp_result.symbol_size, cpp_result.symbols_count,
        );
    }
}

#[test]
fn test_4mb_large_symbol_source_identical() {
    skip_if_no_cpp!();
    let _timeout = TestTimeout::new(300);
    let mut cpp = CppTestNode::spawn(BASE_PORT + 43).expect("spawn C++ node");

    let data = make_random_data(DATA_4MB);

    for &sym_count in SYMBOL_COUNTS {
        let sym_size = symbol_size_for(data.len(), sym_count);
        let (rust_params, rust_symbols) = rust_encode(&data, Some(sym_size), 0);
        let cpp_result = cpp.raptorq_encode(&data, sym_size, 0).expect("C++ encode failed");

        let n = rust_params.symbols_count as usize;
        assert_eq!(n, cpp_result.symbols_count as usize);

        for i in 0..n {
            let rs = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &rust_symbols[i].data,
            )
            .expect("bad base64");
            let cs = base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                &cpp_result.symbols[i].data,
            )
            .expect("bad base64");

            let rs_hash = hash_hex(&rs);
            let cs_hash = hash_hex(&cs);
            assert_eq!(
                rs_hash, cs_hash,
                "source symbol {} differs for {} symbols (sym_size={}): rust={} cpp={}",
                i, sym_count, sym_size, rs_hash, cs_hash,
            );
        }
        println!(
            "OK: {} source symbols identical for {} symbols (sym_size={} = {} KB)",
            n,
            sym_count,
            sym_size,
            sym_size / 1024,
        );
    }
}
