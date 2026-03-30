/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Cross-implementation tests for candidate_id_to_sign bytes.
//!
//! Verifies Rust and C++ build identical TL bytes for:
//! consensus.candidateId slot:int hash:int256

use compat_test::{skip_if_no_cpp, CppTestNode};
use ton_api::{serialize_boxed, ton::consensus, IntoBoxed};
use ton_block::UInt256;

fn parse_uint256(hex_hash: &str) -> UInt256 {
    let bytes = hex::decode(hex_hash).expect("hex decode failed");
    let arr: [u8; 32] = bytes.try_into().expect("hash must be exactly 32 bytes");
    UInt256::with_array(arr)
}

fn rust_candidate_id_to_sign(slot: i32, hex_hash: &str) -> Vec<u8> {
    let hash = parse_uint256(hex_hash);
    let candidate_id = consensus::candidateid::CandidateId { slot, hash };
    serialize_boxed(&candidate_id.into_boxed()).expect("serialize candidateId")
}

fn rust_candidate_parent_wrapped(slot: i32, hex_hash: &str) -> Vec<u8> {
    let hash = parse_uint256(hex_hash);
    let candidate_id = consensus::candidateid::CandidateId { slot, hash };
    let parent = consensus::candidateparent::CandidateParent {
        id: consensus::CandidateId::Consensus_CandidateId(candidate_id),
    };
    serialize_boxed(&parent.into_boxed()).expect("serialize candidateParent")
}

#[test]
fn test_candidate_id_to_sign_matches_cpp() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(15900).expect("spawn C++");
    let cases: &[(i32, &str)] = &[
        (0, "0000000000000000000000000000000000000000000000000000000000000000"),
        (1, "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"),
        (17, "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        (777, "abcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcdabcd"),
    ];

    for (slot, hash_hex) in cases {
        let cpp_bytes =
            cpp.compute_candidate_id_to_sign(*slot, hash_hex).expect("cpp compute candidate id");
        let rust_bytes = rust_candidate_id_to_sign(*slot, hash_hex);
        assert_eq!(
            cpp_bytes, rust_bytes,
            "candidateId bytes mismatch for slot={} hash={}",
            slot, hash_hex
        );
    }

    cpp.shutdown().expect("shutdown");
}

#[test]
fn test_candidate_id_to_sign_not_candidate_parent() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(15901).expect("spawn C++");
    let slot = 42;
    let hash_hex = "1111111111111111111111111111111111111111111111111111111111111111";

    let cpp_bytes =
        cpp.compute_candidate_id_to_sign(slot, hash_hex).expect("cpp compute candidate id");
    let rust_candidate_id = rust_candidate_id_to_sign(slot, hash_hex);
    let rust_parent_wrapped = rust_candidate_parent_wrapped(slot, hash_hex);

    assert_eq!(cpp_bytes, rust_candidate_id);
    assert_ne!(
        cpp_bytes, rust_parent_wrapped,
        "C++ side must sign candidateId directly, not candidateParent wrapper"
    );

    cpp.shutdown().expect("shutdown");
}
