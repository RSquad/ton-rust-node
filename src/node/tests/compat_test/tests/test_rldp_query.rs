/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! RLDP query/response cross-implementation compatibility tests.
//!
//! Tests verify that Rust and C++ nodes can exchange RLDP queries and receive
//! correct echo answers, for both RLDP v1 and v2 protocols.
//!
//! Topology: 2 nodes (sender + responder), echo handler on responder side.
//! The sender sends a query, the responder echoes it back, and the sender
//! verifies the answer matches the original data.
//!
//! Important: All query data is wrapped in a `tonNode.data` TL envelope because
//! the Rust overlay requires valid TL-serialized objects in the query bundle
//! (overlay.query prefix + TL-serialized inner object). The C++ overlay treats
//! query data as opaque bytes, but Rust's `deserialize_boxed_bundle` must
//! successfully parse both TL objects.

use compat_test::{
    skip_if_no_cpp,
    test_helpers::{EchoConsumer, RustTestNode},
    CppTestNode,
};
use std::{thread::sleep, time::Duration};
use ton_api::{
    deserialize_boxed, serialize_boxed,
    ton::ton_node::{data::Data as TonNodeData, Data as TonNodeDataBoxed},
    IntoBoxed,
};

/// Port base for RLDP query tests (each test offsets by 10)
const PORT_BASE: u16 = 15800;

/// Wrap test data in tonNode.data TL envelope and serialize to bytes.
/// This is needed because overlay RLDP queries must carry valid TL objects.
fn wrap_in_tl(data: &[u8]) -> Vec<u8> {
    let tl_data = TonNodeData { data: data.to_vec().into() };
    serialize_boxed(&tl_data.into_boxed()).expect("serialize tonNode.data")
}

/// Extract inner data from a TL-serialized tonNode.data response.
fn unwrap_from_tl(tl_bytes: &[u8]) -> Vec<u8> {
    let obj = deserialize_boxed(tl_bytes).expect("deserialize TL answer");
    match obj.downcast::<TonNodeDataBoxed>() {
        Ok(data) => data.only().data.to_vec(),
        Err(obj) => panic!("Unexpected TL type in answer: {:?}", obj),
    }
}

// ---------------------------------------------------------------------------
// RLDP v1 tests
// ---------------------------------------------------------------------------

/// Test: RLDP v1 query from Rust to C++ (Rust → C++)
/// Rust sends an RLDP query, C++ echoes it back.
#[test]
fn test_rldp_v1_rust_to_cpp() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE;
    let rust_port = PORT_BASE + 1;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    // Compute overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // Get ADNL IDs
    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    println!("Rust ADNL ID: {}", rust_id);
    println!("C++  ADNL ID: {}", cpp_id);

    // Create private overlay on C++ (echo handler is default)
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Add overlay on Rust side
    rust_node.add_public_overlay(&overlay_short_id);

    // Set echo query handler on C++
    cpp.set_query_handler(&cpp_overlay_id, "echo").expect("set query handler");

    // Exchange ADNL peers
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Get C++ key id for targeting
    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send RLDP v1 query from Rust to C++
    // send_rldp_query wraps in tonNode.data + overlay.query prefix internally
    let test_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
    println!("Sending RLDP v1 query from Rust to C++ ({} bytes)", test_data.len());

    let answer = rust_node.send_rldp_query(
        &overlay_short_id,
        &cpp_key_id,
        &test_data,
        1 << 20, // 1MB max answer
        false,   // v1
    );

    assert!(answer.is_some(), "RLDP v1 Rust→C++ query got no answer");
    let answer_bytes = answer.unwrap();
    println!("Got answer: {} bytes (TL-wrapped)", answer_bytes.len());

    // C++ echo handler returns the raw bytes it received (= TL-serialized tonNode.data)
    // Unwrap the TL envelope to get the original data
    let answer_data = unwrap_from_tl(&answer_bytes);
    assert_eq!(answer_data, test_data, "RLDP v1 Rust→C++ echo mismatch");

    println!("PASS: RLDP v1 Rust→C++ query/response works");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: RLDP v1 query from C++ to Rust (C++ → Rust)
/// C++ sends an RLDP query, Rust echoes it back via EchoConsumer.
#[test]
fn test_rldp_v1_cpp_to_rust() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 10;
    let rust_port = PORT_BASE + 11;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    // Compute overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // Get ADNL IDs
    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    println!("Rust ADNL ID: {}", rust_id);
    println!("C++  ADNL ID: {}", cpp_id);

    // Create private overlay on C++
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Add overlay on Rust side and register echo consumer
    rust_node.add_public_overlay(&overlay_short_id);
    rust_node.register_consumer(&overlay_short_id, EchoConsumer::new());

    // Exchange ADNL peers
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send RLDP v1 query from C++ to Rust
    // Pre-wrap data in TL so Rust's deserialize_boxed_bundle can parse it
    let test_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
    let tl_data = wrap_in_tl(&test_data);
    println!(
        "Sending RLDP v1 query from C++ to Rust ({} bytes, {} TL-wrapped)",
        test_data.len(),
        tl_data.len()
    );

    let answer = cpp
        .send_rldp_query(&cpp_overlay_id, &rust_id, &tl_data, 1 << 20, false)
        .expect("C++ send_rldp_query failed");

    println!("Got answer: {} bytes", answer.len());
    // Rust EchoConsumer echoes back the TLObject, which gets TL-serialized
    let answer_data = unwrap_from_tl(&answer);
    assert_eq!(answer_data, test_data, "RLDP v1 C++→Rust echo mismatch");

    println!("PASS: RLDP v1 C++→Rust query/response works");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

// ---------------------------------------------------------------------------
// RLDP v2 tests
// ---------------------------------------------------------------------------

/// Test: RLDP v2 query from Rust to C++ (Rust → C++)
/// Same as v1 test but using RLDP v2 (BBR congestion control, selective ACKs).
#[test]
fn test_rldp_v2_rust_to_cpp() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 20;
    let rust_port = PORT_BASE + 21;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    // Compute overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // Get ADNL IDs
    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    println!("Rust ADNL ID: {}", rust_id);
    println!("C++  ADNL ID: {}", cpp_id);

    // Create private overlay on C++
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Add overlay on Rust side
    rust_node.add_public_overlay(&overlay_short_id);

    // Set echo query handler on C++
    cpp.set_query_handler(&cpp_overlay_id, "echo").expect("set query handler");

    // Exchange ADNL peers
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Get C++ key id for targeting
    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send RLDP v2 query from Rust to C++
    let test_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
    println!("Sending RLDP v2 query from Rust to C++ ({} bytes)", test_data.len());

    let answer = rust_node.send_rldp_query(
        &overlay_short_id,
        &cpp_key_id,
        &test_data,
        1 << 20, // 1MB max answer
        true,    // v2
    );

    assert!(answer.is_some(), "RLDP v2 Rust→C++ query got no answer");
    let answer_bytes = answer.unwrap();
    println!("Got answer: {} bytes (TL-wrapped)", answer_bytes.len());
    let answer_data = unwrap_from_tl(&answer_bytes);
    assert_eq!(answer_data, test_data, "RLDP v2 Rust→C++ echo mismatch");

    println!("PASS: RLDP v2 Rust→C++ query/response works");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: RLDP v2 query from C++ to Rust (C++ → Rust)
/// C++ sends an RLDP v2 query, Rust echoes it back via EchoConsumer.
#[test]
fn test_rldp_v2_cpp_to_rust() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 30;
    let rust_port = PORT_BASE + 31;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    // Compute overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // Get ADNL IDs
    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    println!("Rust ADNL ID: {}", rust_id);
    println!("C++  ADNL ID: {}", cpp_id);

    // Create private overlay on C++
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Add overlay on Rust side and register echo consumer
    rust_node.add_public_overlay(&overlay_short_id);
    rust_node.register_consumer(&overlay_short_id, EchoConsumer::new());

    // Exchange ADNL peers
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send RLDP v2 query from C++ to Rust
    let test_data: Vec<u8> = (0..256).map(|i| (i % 256) as u8).collect();
    let tl_data = wrap_in_tl(&test_data);
    println!(
        "Sending RLDP v2 query from C++ to Rust ({} bytes, {} TL-wrapped)",
        test_data.len(),
        tl_data.len()
    );

    let answer = cpp
        .send_rldp_query(&cpp_overlay_id, &rust_id, &tl_data, 1 << 20, true)
        .expect("C++ send_rldp_query v2 failed");

    println!("Got answer: {} bytes", answer.len());
    let answer_data = unwrap_from_tl(&answer);
    assert_eq!(answer_data, test_data, "RLDP v2 C++→Rust echo mismatch");

    println!("PASS: RLDP v2 C++→Rust query/response works");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

// ---------------------------------------------------------------------------
// Larger payload tests (multi-symbol FEC, RLDP v2 only)
// ---------------------------------------------------------------------------
// 256-byte tests above fit in a single 768-byte FEC symbol.
// These tests exercise multi-symbol RaptorQ encoding/decoding (4KB ≈ 6 symbols).
//
// IMPORTANT: These tests must use RLDP v2 because of MTU limits:
// - C++ RLDP v1 default_mtu_ = 1024 bytes (adnl::Adnl::get_mtu()) — drops incoming
//   transfers with total_size > 1024 unless pre-registered via max_size_ or set_default_mtu
// - C++ RLDP v2 DEFAULT_MTU = 7680 bytes (RldpConnection::DEFAULT_MTU) — allows larger
//   unsolicited transfers, sufficient for our test payloads
// - Rust RLDP has no incoming MTU check (accepts any size)
// - In production, C++ uses PeersMtuLimitGuard to raise limits to max_block_size + 1024

/// Test: 4KB RLDP v2 query from Rust to C++ (multi-symbol FEC)
#[test]
fn test_rldp_v2_4kb_rust_to_cpp() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 40;
    let rust_port = PORT_BASE + 41;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    rust_node.add_public_overlay(&overlay_short_id);
    cpp.set_query_handler(&cpp_overlay_id, "echo").expect("set query handler");

    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);
    sleep(Duration::from_millis(500));

    // 4096 bytes ≈ 6 FEC symbols (768 bytes each)
    let test_data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
    println!("Sending RLDP v2 4KB query from Rust to C++ ({} bytes)", test_data.len());

    let answer = rust_node.send_rldp_query(
        &overlay_short_id,
        &cpp_key_id,
        &test_data,
        1 << 20,
        true, // v2 — required for 4KB (RLDP v1 default_mtu=1024 would reject)
    );

    assert!(answer.is_some(), "RLDP v2 4KB Rust→C++ query got no answer");
    let answer_bytes = answer.unwrap();
    println!("Got answer: {} bytes (TL-wrapped)", answer_bytes.len());
    let answer_data = unwrap_from_tl(&answer_bytes);
    assert_eq!(answer_data, test_data, "RLDP v2 4KB Rust→C++ echo mismatch");

    println!("PASS: RLDP v2 4KB Rust→C++ query/response works");
    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: 4KB RLDP v2 query from C++ to Rust (multi-symbol FEC)
#[test]
fn test_rldp_v2_4kb_cpp_to_rust() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 50;
    let rust_port = PORT_BASE + 51;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    rust_node.add_public_overlay(&overlay_short_id);
    rust_node.register_consumer(&overlay_short_id, EchoConsumer::new());

    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_millis(500));

    // 4096 bytes + TL wrapper ≈ 4104 bytes (under C++ 8192-byte overlay limit)
    let test_data: Vec<u8> = (0..4096).map(|i| (i % 256) as u8).collect();
    let tl_data = wrap_in_tl(&test_data);
    println!(
        "Sending RLDP v2 4KB query from C++ to Rust ({} bytes, {} TL-wrapped)",
        test_data.len(),
        tl_data.len()
    );

    let answer = cpp
        .send_rldp_query(&cpp_overlay_id, &rust_id, &tl_data, 1 << 20, true)
        .expect("C++ send_rldp_query v2 4KB failed");

    println!("Got answer: {} bytes", answer.len());
    let answer_data = unwrap_from_tl(&answer);
    assert_eq!(answer_data, test_data, "RLDP v2 4KB C++→Rust echo mismatch");

    println!("PASS: RLDP v2 4KB C++→Rust query/response works");
    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Near-limit (7KB) RLDP v2 query from Rust to C++ (multi-symbol FEC)
/// 7168 bytes → rldp.query total ≈ 7300 bytes (under RLDP v2 DEFAULT_MTU=7680)
#[test]
fn test_rldp_v2_7kb_rust_to_cpp() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 60;
    let rust_port = PORT_BASE + 61;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    rust_node.add_public_overlay(&overlay_short_id);
    cpp.set_query_handler(&cpp_overlay_id, "echo").expect("set query handler");

    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);
    sleep(Duration::from_millis(500));

    // 7168 bytes ≈ 10 FEC symbols, near RLDP v2 DEFAULT_MTU limit
    let test_data: Vec<u8> = (0..7168).map(|i| (i % 256) as u8).collect();
    println!("Sending RLDP v2 7KB query from Rust to C++ ({} bytes)", test_data.len());

    let answer = rust_node.send_rldp_query(
        &overlay_short_id,
        &cpp_key_id,
        &test_data,
        1 << 20,
        true, // v2
    );

    assert!(answer.is_some(), "RLDP v2 7KB Rust→C++ query got no answer");
    let answer_bytes = answer.unwrap();
    println!("Got answer: {} bytes (TL-wrapped)", answer_bytes.len());
    let answer_data = unwrap_from_tl(&answer_bytes);
    assert_eq!(answer_data, test_data, "RLDP v2 7KB Rust→C++ echo mismatch");

    println!("PASS: RLDP v2 7KB Rust→C++ query/response works");
    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Near-limit (7KB) RLDP v2 query from C++ to Rust (multi-symbol FEC)
#[test]
fn test_rldp_v2_7kb_cpp_to_rust() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 70;
    let rust_port = PORT_BASE + 71;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, true);

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    rust_node.add_public_overlay(&overlay_short_id);
    rust_node.register_consumer(&overlay_short_id, EchoConsumer::new());

    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_millis(500));

    // 7168 bytes + TL wrapper ≈ 7176 bytes (under C++ 8192-byte overlay limit)
    let test_data: Vec<u8> = (0..7168).map(|i| (i % 256) as u8).collect();
    let tl_data = wrap_in_tl(&test_data);
    println!(
        "Sending RLDP v2 7KB query from C++ to Rust ({} bytes, {} TL-wrapped)",
        test_data.len(),
        tl_data.len()
    );

    let answer = cpp
        .send_rldp_query(&cpp_overlay_id, &rust_id, &tl_data, 1 << 20, true)
        .expect("C++ send_rldp_query v2 7KB failed");

    println!("Got answer: {} bytes", answer.len());
    let answer_data = unwrap_from_tl(&answer);
    assert_eq!(answer_data, test_data, "RLDP v2 7KB C++→Rust echo mismatch");

    println!("PASS: RLDP v2 7KB C++→Rust query/response works");
    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

// Note on payload size limits:
//
// RLDP v1 default_mtu = 1024 bytes (total transfer size for unsolicited incoming):
//   - Only ~928 bytes of user data fits (after rldp.query + overlay.query + tonNode.data overhead)
//   - Production nodes call set_default_mtu() to raise this
//
// RLDP v2 DEFAULT_MTU = 7680 bytes:
//   - Up to ~7.5KB user data fits without configuration
//   - Production nodes use PeersMtuLimitGuard to raise to max_block_size + 1024
//
// C++ overlay CHECK: query.size() <= huge_packet_max_size() (8192 bytes):
//   - Hard limit on data passed to Overlays::send_query_via before RLDP wrapping
//   - Applies to C++ sender side only
//
// Rust RaptorQ NEON alignment bug (aarch64 only):
//   - Pre-existing bug triggered by certain larger payload sizes
//   - Not related to cross-implementation compatibility
