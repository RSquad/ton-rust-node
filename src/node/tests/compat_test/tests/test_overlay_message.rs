/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Overlay point-to-point message delivery tests between Rust and C++ implementations.
//!
//! These tests verify that Rust and C++ nodes can exchange overlay messages
//! (not broadcasts) through overlays. This is the same path used by
//! simplex consensus for sending votes and certificates.
//!
//! The key difference from broadcast tests:
//! - Broadcasts use overlay.broadcast() / Overlays::send_broadcast_ex()
//! - Messages use overlay.message() / Overlays::send_message()

use compat_test::{
    skip_if_no_cpp,
    test_helpers::{MessageCollector, RustTestNode},
    CppTestNode,
};
use std::{thread::sleep, time::Duration};

/// Port base for overlay message tests
const PORT_BASE: u16 = 15400;

/// Test: Send overlay message from C++ to Rust (C++ → Rust)
/// C++ uses Overlays::send_message(), Rust receives via overlay consumer callback.
#[test]
fn test_overlay_message_cpp_to_rust() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE;
    let rust_port = PORT_BASE + 1;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, false);

    // Compute overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // Get both ADNL IDs
    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    println!("Rust ADNL ID: {}", rust_id);
    println!("C++  ADNL ID: {}", cpp_id);

    // Create private overlay on C++
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Add true private overlay on Rust side with C++ peer in the member list.
    // Must be private overlay — the overlay dispatcher only calls try_consume_custom
    // on the consumer for private overlays.
    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);
    rust_node.add_true_private_overlay(&overlay_short_id, &[cpp_key_id]);
    let collector = MessageCollector::new();
    rust_node.overlay.add_consumer(&overlay_short_id, collector.clone()).expect("add consumer");

    // Exchange ADNL peers (just ADNL level, overlay peers are set at creation time)
    rust_node.add_cpp_peer(&cpp);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send overlay message from C++ to Rust
    let test_data = b"Hello from C++ overlay message";
    cpp.send_message(&cpp_overlay_id, &rust_id, test_data).expect("C++ send message");

    println!("C++ sent overlay message ({} bytes) to Rust", test_data.len());

    // Wait for Rust to receive via MessageCollector
    let received = collector.wait_for_messages(&rust_node.rt, 1, 5);

    assert!(!received.is_empty(), "C++->Rust overlay message was NOT delivered");
    assert_eq!(received[0], test_data, "Message data mismatch");
    println!("C++->Rust overlay message delivered and verified!");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Send overlay message from Rust to C++ (Rust → C++)
/// This is the critical path: Rust overlay.message() → C++ receive_message callback.
/// This is what simplex consensus uses to send votes and certificates.
#[test]
fn test_overlay_message_rust_to_cpp() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 10;
    let rust_port = PORT_BASE + 11;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, false);

    // Compute overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // Get both ADNL IDs
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

    // Exchange ADNL peers
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Get C++ key id for targeting
    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send overlay message from Rust to C++ using Fast (UDP) method
    let test_data = b"Hello from Rust overlay message (Fast/UDP)";
    println!("Sending overlay message from Rust to C++ ({} bytes, Fast/UDP)", test_data.len());
    rust_node.send_message(&overlay_short_id, &cpp_key_id, test_data);

    // Wait for C++ to receive
    sleep(Duration::from_secs(2));

    // Check C++ received messages
    let received = cpp.get_received_messages(&cpp_overlay_id).expect("get messages");

    println!("C++ received {} overlay messages", received.len());
    for (i, msg) in received.iter().enumerate() {
        println!("  msg[{}]: source={}, size={}", i, msg.source, msg.size);
    }

    assert!(!received.is_empty(), "Rust->C++ overlay message was NOT delivered via Fast/UDP");
    assert_eq!(received[0].size, test_data.len(), "Message size mismatch");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Send multiple overlay messages from Rust to C++ and check delivery rate
/// This simulates the real consensus scenario where many small messages are sent.
#[test]
fn test_overlay_message_burst_rust_to_cpp() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 30;
    let rust_port = PORT_BASE + 31;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, false);

    // Compute overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    // Get both ADNL IDs
    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    // Create private overlay on C++
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    // Add overlay on Rust side
    rust_node.add_public_overlay(&overlay_short_id);

    // Exchange ADNL peers
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    // Get C++ key id for targeting
    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_secs(1));

    // Send a burst of messages (simulating consensus votes)
    let num_messages = 20;
    println!("Sending {} overlay messages from Rust to C++ (Fast/UDP)", num_messages);

    for i in 0..num_messages {
        let msg = format!("vote_message_{:04}", i);
        rust_node.send_message(&overlay_short_id, &cpp_key_id, msg.as_bytes());
    }

    // Wait for delivery
    sleep(Duration::from_secs(3));

    // Check delivery rate
    let received = cpp.get_received_messages(&cpp_overlay_id).expect("get messages");

    println!(
        "Delivery rate: {}/{} ({:.1}%)",
        received.len(),
        num_messages,
        (received.len() as f64 / num_messages as f64) * 100.0
    );

    for (i, msg) in received.iter().enumerate() {
        println!("  msg[{}]: source={}, size={}", i, msg.source, msg.size);
    }

    // Require at least 90% delivery — UDP on localhost should be reliable.
    // Anything less indicates a real problem, not normal network loss.
    let min_required = (num_messages as f64 * 0.9) as usize;
    assert!(
        received.len() >= min_required,
        "Too many messages lost: {}/{} delivered ({:.1}% loss, need >=90%)",
        received.len(),
        num_messages,
        (1.0 - received.len() as f64 / num_messages as f64) * 100.0
    );

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: Send overlay message from C++ to C++ (C++ → C++) as baseline
/// This confirms C++ send_message works when both sides are C++.
#[test]
fn test_overlay_message_cpp_to_cpp_baseline() {
    skip_if_no_cpp!();

    let cpp1_port = PORT_BASE + 40;
    let cpp2_port = PORT_BASE + 41;

    let mut cpp1 = CppTestNode::spawn(cpp1_port).expect("spawn C++ node 1");
    let mut cpp2 = CppTestNode::spawn(cpp2_port).expect("spawn C++ node 2");

    // Use a simple overlay name for testing
    let overlay_name = b"test_message_overlay_cpp2cpp";

    let cpp1_id = cpp1.adnl_id().to_string();
    let cpp2_id = cpp2.adnl_id().to_string();

    println!("C++ node 1 ADNL ID: {}", cpp1_id);
    println!("C++ node 2 ADNL ID: {}", cpp2_id);

    // Create private overlay on both C++ nodes
    let overlay_id_1 = cpp1
        .create_private_overlay(overlay_name, vec![cpp1_id.clone(), cpp2_id.clone()])
        .expect("C++ 1 create private overlay");
    let overlay_id_2 = cpp2
        .create_private_overlay(overlay_name, vec![cpp1_id.clone(), cpp2_id.clone()])
        .expect("C++ 2 create private overlay");

    assert_eq!(overlay_id_1, overlay_id_2, "Overlay IDs should match");

    // Exchange peers
    let cpp1_pubkey = cpp1.pubkey().to_string();
    let cpp2_pubkey = cpp2.pubkey().to_string();
    cpp1.add_peer(&cpp2_pubkey, "127.0.0.1", cpp2_port).expect("C++ 1 add peer");
    cpp2.add_peer(&cpp1_pubkey, "127.0.0.1", cpp1_port).expect("C++ 2 add peer");

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send overlay message from C++ node 1 to C++ node 2
    let test_data = b"C++ to C++ overlay message";
    cpp1.send_message(&overlay_id_1, &cpp2_id, test_data).expect("C++ 1 send message");

    // Wait for delivery
    sleep(Duration::from_secs(2));

    // Check if C++ node 2 received
    let received = cpp2.get_received_messages(&overlay_id_2).expect("get messages");

    println!("C++ node 2 received {} overlay messages", received.len());

    assert!(!received.is_empty(), "C++->C++ overlay message was NOT delivered (baseline test)");
    assert_eq!(received[0].size, test_data.len(), "Message size mismatch");

    cpp1.shutdown().expect("shutdown node 1");
    cpp2.shutdown().expect("shutdown node 2");
}
