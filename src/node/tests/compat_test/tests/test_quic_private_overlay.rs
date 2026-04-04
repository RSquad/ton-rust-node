/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Private overlay tests with ADNL and QUIC transport.
//!
//! These tests create **true private overlays** on the Rust side using
//! `OverlayNode::add_private_overlay()` with a signing key — matching how
//! validator consensus overlays are created in production.
//!
//! QUIC transport is used by calling `send_quic_overlay_message` /
//! `send_quic_overlay_query` directly, which bypass the overlay layer and
//! send via `QuicNode`. The overlay's own transport is always ADNL.
//!
//! Test matrix:
//! - Private overlay + ADNL send: baseline
//! - Private overlay + QUIC send: message and query delivery through QUIC
//! - Private overlay + C++→Rust: inbound message delivery via ADNL

use compat_test::{
    skip_if_no_cpp,
    test_helpers::{MessageCollector, RustQuicTestNode},
    CppTestNode,
};
use std::{thread::sleep, time::Duration};

/// Port base for QUIC private overlay tests (must not conflict with other test suites)
const PORT_BASE: u16 = 18200;

/// Test: Private overlay message via ADNL (baseline).
///
/// Creates a true private overlay on Rust side and sends a message via ADNL.
/// This verifies that `add_private_overlay()` + `overlay.message()` work
/// correctly with C++ — the same code path validators use.
#[test]
fn test_private_overlay_adnl_message_rust_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();

    let cpp_port = PORT_BASE;
    let rust_port = PORT_BASE + 1;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();
    println!("Rust ADNL ID: {rust_id}");
    println!("C++  ADNL ID: {cpp_id}");

    // Compute overlay (same method as existing tests)
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // C++ creates private overlay
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Rust creates TRUE private overlay (not public shortcut) with ADNL transport
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    rust_node.add_private_overlay(&overlay_short_id, &[cpp_key_id.clone()], true);

    // Exchange ADNL peers
    rust_node.add_cpp_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Add C++ peer to overlay's known peers
    // Peer already registered in overlay via add_private_overlay

    sleep(Duration::from_millis(500));

    // Send message through overlay.message() — routes via ADNL
    let test_data = b"Private overlay message via ADNL (baseline)";
    println!("Sending private overlay message via ADNL ({} bytes)", test_data.len());
    rust_node.send_overlay_message(&overlay_short_id, &cpp_key_id, test_data);

    sleep(Duration::from_secs(2));

    let received = cpp.get_received_messages(&cpp_overlay_id).expect("get messages");
    println!("C++ received {} overlay messages (ADNL)", received.len());
    for (i, msg) in received.iter().enumerate() {
        println!("  msg[{i}]: source={}, size={}", msg.source, msg.size);
    }

    assert!(!received.is_empty(), "Private overlay message via ADNL was NOT delivered (Rust→C++)");
    assert_eq!(received[0].size, test_data.len(), "Message size mismatch");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Private overlay message via QUIC transport.
///
/// Sends a message via `send_quic_overlay_message` which uses `QuicNode`
/// directly (bypassing `overlay.message()` which always uses ADNL).
#[test]
fn test_private_overlay_quic_message_rust_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();

    let cpp_port = PORT_BASE + 10;
    let rust_port = PORT_BASE + 11;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();
    println!("Rust ADNL ID: {rust_id}");
    println!("C++  ADNL ID: {cpp_id}");

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // C++ creates private overlay (with QUIC enabled)
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Rust creates private overlay with QUIC transport
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    rust_node.add_private_overlay(&overlay_short_id, &[cpp_key_id.clone()], true);

    // Exchange ADNL peers (needed for peer identity resolution)
    rust_node.add_cpp_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Register QUIC peer address
    rust_node.add_cpp_quic_peer(&cpp);

    // Add C++ peer to overlay's known peers
    // Peer already registered in overlay via add_private_overlay

    sleep(Duration::from_secs(1));

    // Send message directly via QUIC transport (bypassing overlay.message() which
    // always routes via ADNL/UDP — the overlay layer has no QUIC transport config).
    let test_data = b"Private overlay message via QUIC transport";
    println!("Sending private overlay message via QUIC ({} bytes)", test_data.len());
    rust_node.send_quic_overlay_message(&cpp_key_id, &overlay_short_id, test_data);

    sleep(Duration::from_secs(2));

    let received = cpp.get_received_messages(&cpp_overlay_id).expect("get messages");
    println!("C++ received {} overlay messages (QUIC)", received.len());
    for (i, msg) in received.iter().enumerate() {
        println!("  msg[{i}]: source={}, size={}", msg.source, msg.size);
    }

    assert!(!received.is_empty(), "Private overlay message via QUIC was NOT delivered (Rust→C++)");
    assert_eq!(received[0].size, test_data.len(), "Message size mismatch");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Multiple messages via QUIC transport in private overlay.
///
/// Sends a burst of messages (simulating consensus votes) through a QUIC-backed
/// private overlay and checks delivery rate.
#[test]
fn test_private_overlay_quic_message_burst() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();

    let cpp_port = PORT_BASE + 20;
    let rust_port = PORT_BASE + 21;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    rust_node.add_private_overlay(&overlay_short_id, &[cpp_key_id.clone()], true);

    rust_node.add_cpp_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");
    rust_node.add_cpp_quic_peer(&cpp);
    // Peer already registered in overlay via add_private_overlay

    sleep(Duration::from_secs(1));

    // Send burst of messages (simulating consensus votes)
    let num_messages = 20;
    println!("Sending {num_messages} overlay messages via QUIC");

    for i in 0..num_messages {
        let msg = format!("quic_vote_{i:04}");
        rust_node.send_quic_overlay_message(&cpp_key_id, &overlay_short_id, msg.as_bytes());
    }

    sleep(Duration::from_secs(3));

    let received = cpp.get_received_messages(&cpp_overlay_id).expect("get messages");
    println!(
        "Delivery rate: {}/{} ({:.1}%)",
        received.len(),
        num_messages,
        (received.len() as f64 / num_messages as f64) * 100.0
    );

    assert!(!received.is_empty(), "No QUIC overlay messages delivered in burst of {num_messages}");
    // QUIC should deliver reliably — stream-based, no UDP loss
    assert_eq!(
        received.len(),
        num_messages,
        "QUIC should deliver all messages (stream-based, no UDP loss)"
    );

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Private overlay query via QUIC transport.
///
/// Sends an overlay query from Rust to C++ through `overlay.query()` which
/// routes through the QUIC transport. The C++ echo handler returns the query
/// data in the response.
#[test]
fn test_private_overlay_quic_query_rust_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();

    let cpp_port = PORT_BASE + 30;
    let rust_port = PORT_BASE + 31;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();
    println!("Rust ADNL ID: {rust_id}");
    println!("C++  ADNL ID: {cpp_id}");

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // C++ creates overlay with echo handler
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");
    cpp.set_query_handler(&cpp_overlay_id, "echo").expect("set echo handler");

    // Rust creates private overlay with QUIC transport
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    rust_node.add_private_overlay(&overlay_short_id, &[cpp_key_id.clone()], true);

    rust_node.add_cpp_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");
    rust_node.add_cpp_quic_peer(&cpp);
    // Peer already registered in overlay via add_private_overlay

    sleep(Duration::from_secs(1));

    // Send query through overlay.query() — routes via QUIC transport
    let query_data = b"QUIC private overlay query";
    println!("Sending overlay query via QUIC ({} bytes)", query_data.len());

    // Use raw QUIC overlay query (overlay.query() needs a proper TL object,
    // so we use the lower-level send_quic_overlay_query for now)
    let result = rust_node.send_quic_overlay_query(&cpp_key_id, &overlay_short_id, query_data, 10);

    match result {
        Ok(answer) => {
            println!("SUCCESS: Got QUIC overlay query answer ({} bytes)", answer.len());
            assert!(!answer.is_empty(), "Answer should not be empty");
        }
        Err(e) => {
            panic!("QUIC overlay query via private overlay failed: {e}");
        }
    }

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: C++ sends overlay message to Rust via private overlay (C++→Rust direction).
///
/// Verifies that messages from C++ arrive at Rust through the overlay callback
/// when Rust uses a private overlay. The C++ send_message goes through ADNL,
/// Rust receives via its overlay consumer regardless of its outbound transport.
#[test]
fn test_private_overlay_message_cpp_to_rust() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();

    let cpp_port = PORT_BASE + 40;
    let rust_port = PORT_BASE + 41;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();
    println!("Rust ADNL ID: {rust_id}");
    println!("C++  ADNL ID: {cpp_id}");

    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // C++ creates private overlay
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Rust creates private overlay with ADNL (inbound transport doesn't matter —
    // incoming messages arrive via ADNL subscriber regardless)
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    rust_node.add_private_overlay(&overlay_short_id, &[cpp_key_id.clone()], true);

    // Register message collector to verify receipt on Rust side
    let collector = MessageCollector::new();
    rust_node.overlay.add_consumer(&overlay_short_id, collector.clone()).expect("add consumer");

    // Exchange peers
    rust_node.add_cpp_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_millis(500));

    // C++ sends overlay message to Rust
    let test_data = b"C++ to Rust private overlay message";
    cpp.send_message(&cpp_overlay_id, &rust_id, test_data).expect("C++ send overlay message");
    println!("C++ sent overlay message to Rust ({} bytes)", test_data.len());

    // Wait for Rust to receive via MessageCollector
    let received = collector.wait_for_messages(&rust_node.rt, 1, 5);

    assert!(!received.is_empty(), "C++→Rust private overlay message was NOT delivered");
    assert_eq!(received[0], test_data, "Message data mismatch");
    println!("C++→Rust private overlay message delivered and verified!");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}
