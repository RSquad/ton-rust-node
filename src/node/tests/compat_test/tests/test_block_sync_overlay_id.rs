/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Cross-implementation tests for `consensus.blockSyncOverlayId`.
//!
//! Verifies that Rust and C++ agree on:
//!   1. The boxed-TL seed bytes for the new overlay id.
//!   2. The derived `OverlayIdShort` (SHA256 of `pub.overlay{name = seed}`).
//!   3. The fact that the block-sync overlay short-id is DISTINCT from the
//!      legacy `consensus.overlayId` short-id for the same `session_id`.
//!
//! Without (1) and (2) Rust would join a different overlay than C++ for the
//! same session - the wire is otherwise indistinguishable, so a divergence
//! here would cause silent split-brain.

use compat_test::{overlay_id::compute_overlay_id, skip_if_no_cpp, CppTestNode};
use std::str::FromStr;
use ton_api::{
    serialize_boxed,
    ton::consensus::{
        blocksyncoverlayid::BlockSyncOverlayId, overlayid::OverlayId, BlockOverlayId,
        OverlayId as OverlayIdBoxed,
    },
    IntoBoxed,
};
use ton_block::UInt256;

fn rust_block_sync_seed(session_id_hex: &str) -> Vec<u8> {
    let session_id = UInt256::from_str(session_id_hex).expect("parse session_id hex");
    let boxed: BlockOverlayId = BlockSyncOverlayId { session_id }.into_boxed();
    serialize_boxed(&boxed).expect("serialize blockSyncOverlayId")
}

fn rust_consensus_overlay_seed(session_id_hex: &str) -> Vec<u8> {
    let session_id = UInt256::from_str(session_id_hex).expect("parse session_id hex");
    let boxed: OverlayIdBoxed = OverlayId { session_id, nodes: Vec::new() }.into_boxed();
    serialize_boxed(&boxed).expect("serialize consensus.overlayId")
}

const TEST_VECTORS: &[&str] = &[
    "0000000000000000000000000000000000000000000000000000000000000000",
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    "deadbeefcafebabe0011223344556677889900aabbccddeeff00112233445566",
];

/// Rust- and C++-computed seed bytes for `consensus.blockSyncOverlayId` must
/// be byte-identical, and the SHA256-derived OverlayIdShort must agree.
#[test]
fn test_block_sync_overlay_id_matches_cpp() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(15910).expect("spawn C++");

    for session_id_hex in TEST_VECTORS {
        let (cpp_seed, cpp_short_id_hex) =
            cpp.compute_block_sync_overlay_id(session_id_hex).expect("cpp seed");

        let rust_seed = rust_block_sync_seed(session_id_hex);
        assert_eq!(cpp_seed, rust_seed, "seed bytes mismatch for session_id={}", session_id_hex);

        let rust_short_id = compute_overlay_id(&rust_seed);
        let rust_short_id_hex = hex::encode(rust_short_id);
        assert_eq!(
            cpp_short_id_hex.to_lowercase(),
            rust_short_id_hex,
            "OverlayIdShort mismatch for session_id={}",
            session_id_hex
        );
    }

    cpp.shutdown().expect("shutdown");
}

/// The block-sync overlay short-id MUST differ from the consensus private
/// overlay short-id for the same `session_id`, so the two overlays don't
/// collide.
#[test]
fn test_block_sync_overlay_id_distinct_from_consensus_overlay_id() {
    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(15911).expect("spawn C++");

    let session_id_hex = TEST_VECTORS[2];
    let (cpp_block_sync_seed, cpp_block_sync_short_id) =
        cpp.compute_block_sync_overlay_id(session_id_hex).expect("cpp block-sync");

    // Cross-check: Rust must agree on the block-sync side too.
    let rust_block_sync_seed = rust_block_sync_seed(session_id_hex);
    assert_eq!(cpp_block_sync_seed, rust_block_sync_seed);

    // Now derive the legacy consensus overlay seed/short-id locally.
    let rust_consensus_seed = rust_consensus_overlay_seed(session_id_hex);
    assert_ne!(&cpp_block_sync_seed[..4], &rust_consensus_seed[..4], "constructor IDs must differ");

    let rust_consensus_short_id = hex::encode(compute_overlay_id(&rust_consensus_seed));
    assert_ne!(
        cpp_block_sync_short_id.to_lowercase(),
        rust_consensus_short_id,
        "block-sync and consensus overlays must produce distinct short-ids"
    );

    cpp.shutdown().expect("shutdown");
}

/// Rust and C++ must derive the same sorted-unique ADNL id union from prev|curr|next sets
/// (each id is `addr.is_zero() ? compute_short_id(key) : addr`; C++ `manager.cpp:2440-2461`)
#[test]
fn test_block_sync_overlay_members_matches_cpp() {
    use compat_test::BlockSyncValidatorDescr;
    use ton_block::{SigPubKey, ValidatorDescr, ValidatorSet};

    skip_if_no_cpp!();

    let mut cpp = CppTestNode::spawn(15912).expect("spawn C++");

    fn vdescr(seed: u8, with_addr: bool) -> (ValidatorDescr, BlockSyncValidatorDescr) {
        let key_bytes = [seed; 32];
        let sig_pub = SigPubKey::with_bytes(key_bytes);
        let addr_bytes = [seed ^ 0x80; 32];
        let addr = if with_addr { Some(ton_block::UInt256::from(addr_bytes)) } else { None };
        let descr = ValidatorDescr::with_params(sig_pub, 1, addr);
        let cpp_descr = BlockSyncValidatorDescr {
            key: hex::encode(key_bytes),
            addr: if with_addr { hex::encode(addr_bytes) } else { String::new() },
        };
        (descr, cpp_descr)
    }

    fn vset(entries: &[(u8, bool)]) -> (ValidatorSet, Vec<BlockSyncValidatorDescr>) {
        if entries.is_empty() {
            return (ValidatorSet::default(), vec![]);
        }
        let mut rust_list = Vec::new();
        let mut cpp_list = Vec::new();
        for &(seed, with_addr) in entries {
            let (r, c) = vdescr(seed, with_addr);
            rust_list.push(r);
            cpp_list.push(c);
        }
        (ValidatorSet::new(0, 100, 1, rust_list).unwrap(), cpp_list)
    }

    // 5 test scenarios: empty/overlapping/with-and-without-explicit-addr/etc.
    let scenarios: Vec<(Vec<(u8, bool)>, Vec<(u8, bool)>, Vec<(u8, bool)>, &'static str)> = vec![
        // (prev, curr, next, label)
        (vec![], vec![(1, false), (2, false)], vec![], "current-only-no-addr"),
        (vec![(1, false)], vec![(2, false), (3, false)], vec![(4, false)], "no-overlap"),
        (
            vec![(1, false), (2, false), (3, false)],
            vec![(2, false), (3, false), (4, false)],
            vec![(3, false), (4, false), (5, false)],
            "overlapping",
        ),
        (vec![], vec![(10, true), (11, true)], vec![], "explicit-addrs"),
        (
            vec![(20, false), (21, true)],
            vec![(21, true), (22, false)],
            vec![(22, false), (23, true)],
            "mixed-addr-and-pubkey-derived",
        ),
    ];

    for (prev_seeds, curr_seeds, next_seeds, label) in scenarios {
        let (rust_prev, cpp_prev) = vset(&prev_seeds);
        let (rust_curr, cpp_curr) = vset(&curr_seeds);
        let (rust_next, cpp_next) = vset(&next_seeds);

        // Build the same ConfigParams shape as BlockSyncOverlayParams::from_config expects.
        let mut consensus = ton_block::ConsensusConfig::new();
        consensus.round_candidates = 1;
        consensus.max_block_bytes = 1 << 22;
        consensus.max_collated_bytes = 1 << 22;
        let mut params = ton_block::ConfigParams::default();
        params.set_config(ton_block::ConfigParamEnum::ConfigParam29(consensus)).unwrap();
        if !rust_prev.list().is_empty() {
            params
                .set_config(ton_block::ConfigParamEnum::ConfigParam32(
                    ton_block::ConfigParam32::with_validator_set(rust_prev),
                ))
                .unwrap();
        }
        let rust_curr_for_subset = rust_curr.clone();
        if !rust_curr.list().is_empty() {
            params
                .set_config(ton_block::ConfigParamEnum::ConfigParam34(
                    ton_block::ConfigParam34::with_validator_set(rust_curr),
                ))
                .unwrap();
        }
        if !rust_next.list().is_empty() {
            params
                .set_config(ton_block::ConfigParamEnum::ConfigParam36(
                    ton_block::ConfigParam36::with_validator_set(rust_next),
                ))
                .unwrap();
        }
        let rust = consensus_common::BlockSyncOverlayParams::from_config(
            &params,
            /*slots_per_leader_window*/ 4,
            &rust_curr_for_subset,
        )
        .expect("BlockSyncOverlayParams::from_config");
        let rust_members: Vec<String> =
            rust.members.iter().map(|id| hex::encode(id.data())).collect();

        let cpp_members = cpp
            .compute_block_sync_overlay_members(cpp_prev, cpp_curr, cpp_next)
            .expect("cpp compute_block_sync_overlay_members");
        let cpp_members_lower: Vec<String> =
            cpp_members.into_iter().map(|s| s.to_lowercase()).collect();

        assert_eq!(
            cpp_members_lower, rust_members,
            "block-sync members mismatch for scenario {label}: \
             rust={rust_members:?}, cpp={cpp_members_lower:?}"
        );
    }

    cpp.shutdown().expect("shutdown");
}
