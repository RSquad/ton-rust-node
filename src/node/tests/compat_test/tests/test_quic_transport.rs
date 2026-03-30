/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! QUIC transport compatibility tests between Rust and C++ implementations.
//!
//! Tests raw QUIC query exchange (C++→Rust), large overlay message delivery
//! via QUIC, and QUIC connection establishment (TLS handshake with RPK certs).
//!
//! For overlay-routed QUIC messages/queries (Rust→C++ and C++→Rust), see
//! `test_quic_overlay.rs`. For private overlay QUIC tests, see
//! `test_quic_private_overlay.rs`.

use compat_test::{skip_if_no_cpp, test_helpers::RustQuicTestNode, CppTestNode, TestTimeout};
use std::{
    panic::{catch_unwind, AssertUnwindSafe},
    thread::sleep,
    time::Duration,
};
use ton_api::{serialize_boxed, ton::ton_node::data::Data as TonNodeData, IntoBoxed};

/// Port base for QUIC transport tests
const PORT_BASE: u16 = 18000;

/// Test: C++ sends QUIC query to Rust, expects echo answer
///
/// Expected: PASS — the Rust QUIC server processes the query and echoes it back.
/// Note: Query data must be a valid TL-serialized object because the Rust
/// query processing pipeline deserializes inner data via deserialize_boxed_bundle().
#[test]
fn test_quic_query_cpp_to_rust() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE + 30;
    let rust_port = PORT_BASE + 31;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();

    // Exchange peers
    rust_node.add_cpp_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_millis(500));

    // C++ sends QUIC query to Rust.
    // The inner data must be a valid TL-serialized object because the Rust
    // Query::process() calls deserialize_boxed_bundle() on the raw bytes.
    let payload = b"QUIC query from C++ to Rust";
    let tl_query = TonNodeData { data: payload.to_vec().into() };
    let query_data = serialize_boxed(&tl_query.into_boxed()).expect("serialize query TL");
    println!("C++ sending QUIC query to Rust ({} bytes TL-wrapped)", query_data.len());

    let result = cpp.send_quic_query(&rust_id, &query_data, 5000);

    match result {
        Ok(answer) => {
            println!("SUCCESS: C++ got QUIC query answer ({} bytes)", answer.len());
            // The echo subscriber returns the same TL object; verify it matches
            assert_eq!(answer, query_data, "Query echo data mismatch");
        }
        Err(e) => {
            panic!("C++→Rust QUIC query failed: {}", e);
        }
    }

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Rust sends a QUIC message close to the C++ stream size limit (900 bytes payload).
///
/// The C++ QuicSender enforces a 1024-byte per-stream limit for messages.
/// With overlay prefix (~36 bytes) and quic.message TL wrapper (~8 bytes),
/// 900 bytes of payload stays just under the limit.
/// The message is overlay-routed so C++ can verify receipt.
#[test]
fn test_quic_large_overlay_message_rust_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE + 40;
    let rust_port = PORT_BASE + 41;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    // Create overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    rust_node.add_public_overlay(&overlay_short_id);

    // Exchange peers
    rust_node.add_cpp_peer(&cpp);
    rust_node.add_cpp_quic_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_millis(500));

    // Send 900-byte message with overlay wrapping (under C++ 1024-byte stream limit)
    let large_data: Vec<u8> = (0..900u32).map(|i| (i % 256) as u8).collect();
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    println!("Sending {} byte QUIC overlay message from Rust to C++", large_data.len());
    rust_node.send_quic_overlay_message(&cpp_key_id, &overlay_short_id, &large_data);

    sleep(Duration::from_secs(3));

    let received = cpp.get_received_messages(&cpp_overlay_id).expect("get messages");
    println!("C++ received {} messages", received.len());

    assert!(!received.is_empty(), "QUIC overlay message not received");
    println!("SUCCESS: QUIC message delivered ({} bytes)", received[0].size);

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: QUIC connection establishment between Rust and C++
///
/// Minimal test that just verifies whether a QUIC connection can be established
/// from Rust to C++ (TLS handshake with RPK certificates).
#[test]
fn test_quic_connection_establishment() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE + 50;
    let rust_port = PORT_BASE + 51;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let quic_port = cpp.enable_quic().expect("enable QUIC on C++");
    println!("C++ QUIC port: {}", quic_port);

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);
    println!("Rust QUIC port: {}", rust_port + 1000);

    // Exchange ADNL peers
    rust_node.add_cpp_peer(&cpp);
    rust_node.add_cpp_quic_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_millis(500));

    // Try to send a small message — the connection attempt itself is the test
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    let result = catch_unwind(AssertUnwindSafe(|| {
        rust_node.send_quic_message(&cpp_key_id, b"ping");
    }));

    result.expect("QUIC connection should succeed (Rust → C++)");
    println!("SUCCESS: QUIC connection established (Rust → C++)");
    println!("TLS handshake with RPK certificates succeeded");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}
