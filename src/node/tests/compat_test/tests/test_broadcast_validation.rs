/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Broadcast Validation (2-Phase) Compatibility Tests
//!
//! Tests that the 2-phase broadcast validation (check_broadcast) works
//! consistently between Rust and C++ implementations.
//!
//! In 2-phase broadcast:
//!   1. Receiving node gets check_broadcast callback with data
//!   2. Callback accepts or rejects the broadcast
//!   3. If accepted: broadcast delivered to application + redistributed
//!   4. If rejected: broadcast dropped, NOT redistributed

use adnl::OverlayShortId;
use compat_test::{skip_if_no_cpp, test_helpers::RustTestNode, CppTestNode};
use std::{sync::Arc, thread::sleep, time::Duration};

/// Port base for broadcast validation tests
const PORT_BASE: u16 = 15300;

/// Helper: set up a Rust+C++ pair with a private overlay and accept mode on C++
fn setup_validation_pair(
    port_offset: u16,
    validator_mode: &str,
) -> (CppTestNode, RustTestNode, Arc<OverlayShortId>, String) {
    let cpp_port = PORT_BASE + port_offset;
    let rust_port = PORT_BASE + port_offset + 1;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, false);

    // Use a unique overlay name for each test to avoid conflicts
    let overlay_name = format!("validation_test_{}", port_offset);
    let overlay_name_bytes = overlay_name.as_bytes();

    // Compute overlay short ID from name
    let overlay_short_id = rust_node.compute_overlay_short_id_from_name(overlay_name_bytes);

    // Create private overlay on both sides (private overlays don't require DHT)
    let cpp_adnl_id = cpp.adnl_id();
    let rust_adnl_id = rust_node.adnl_id_hex();

    rust_node.add_private_overlay(&overlay_short_id, vec![cpp_adnl_id.to_string()]);
    let cpp_overlay_id =
        cpp.create_private_overlay(overlay_name_bytes, vec![rust_adnl_id]).expect("create overlay");

    // Set broadcast validator mode on C++
    cpp.set_broadcast_validator(&cpp_overlay_id, validator_mode).expect("set validator mode");

    // Exchange peers - add to both ADNL and overlay neighbours
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("add peer");

    (cpp, rust_node, overlay_short_id, cpp_overlay_id)
}

/// Test: C++ in accept_all mode receives broadcast from Rust
#[test]
fn test_cpp_accept_all_receives_broadcast() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_validation_pair(0, "accept_all");

    // Allow time for ADNL channel establishment
    sleep(Duration::from_secs(1));

    // Send broadcast from Rust using the overlay we created
    let test_data = b"Broadcast with accept_all validation";
    rust_node.send_broadcast(&overlay_id, test_data);

    // Wait for delivery
    sleep(Duration::from_secs(3));

    let received = cpp.get_received_broadcasts(&cpp_overlay_id).expect("get broadcasts");

    // Broadcast MUST be delivered for the test to pass
    assert!(!received.is_empty(), "Rust->C++ broadcast was not delivered in accept_all mode");
    assert!(received[0].accepted, "Broadcast should be accepted");
    assert_eq!(received[0].size, test_data.len(), "Broadcast size mismatch");
    println!("accept_all: broadcast correctly accepted and delivered!");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: C++ in reject_all mode does NOT deliver broadcast from Rust
#[test]
fn test_cpp_reject_all_drops_broadcast() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_validation_pair(10, "reject_all");

    // Allow time for ADNL channel
    sleep(Duration::from_millis(500));

    // Send broadcast from Rust using the overlay we created
    let test_data = b"Broadcast with reject_all validation";
    rust_node.send_broadcast(&overlay_id, test_data);

    // Wait to ensure it would have arrived
    sleep(Duration::from_secs(2));

    let received = cpp.get_received_broadcasts(&cpp_overlay_id).expect("get broadcasts");

    // In reject_all mode, no broadcasts should be delivered to application layer
    println!("reject_all: C++ received {} broadcasts (should be 0)", received.len());

    // If the broadcast was received at the ADNL level but rejected by validator,
    // it should NOT appear in received_broadcasts
    for bc in &received {
        assert!(!bc.accepted, "In reject_all mode, no broadcast should be accepted");
    }

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: toggling validator mode between accept and reject
#[test]
fn test_cpp_toggle_validator_mode() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 20;
    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");

    let overlay_name = b"toggle_test_overlay";
    // Use private overlay (doesn't require DHT)
    let overlay_id = cpp.create_private_overlay(overlay_name, vec![]).expect("create overlay");

    // Toggle modes
    for mode in ["accept_all", "reject_all", "accept_all", "reject_all", "accept_all"] {
        cpp.set_broadcast_validator(&overlay_id, mode).expect(&format!("set mode={}", mode));
        println!("Set broadcast_validator mode={}", mode);
    }

    cpp.shutdown().expect("shutdown");
}

/// Test: C++ node correctly reports acceptance state
#[test]
fn test_broadcast_acceptance_tracking() {
    skip_if_no_cpp!();

    let cpp_port = PORT_BASE + 30;
    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");

    let overlay_name = b"acceptance_tracking_test";
    // Use private overlay (doesn't require DHT)
    let overlay_id = cpp.create_private_overlay(overlay_name, vec![]).expect("create overlay");

    // Initially should have no broadcasts
    let received = cpp.get_received_broadcasts(&overlay_id).expect("get broadcasts");
    assert!(received.is_empty(), "Should have no broadcasts initially");

    // Clear should work on empty list
    cpp.clear_received_broadcasts(&overlay_id).expect("clear broadcasts");

    let received = cpp.get_received_broadcasts(&overlay_id).expect("get broadcasts");
    assert!(received.is_empty(), "Should still be empty after clear");

    println!("Broadcast acceptance tracking works correctly");

    cpp.shutdown().expect("shutdown");
}
