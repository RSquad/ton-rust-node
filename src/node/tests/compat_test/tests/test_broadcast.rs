/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Broadcast compatibility tests between Rust and C++ implementations.
//!
//! Tests that overlay broadcasts sent from one implementation are correctly
//! received by the other. Covers both small (inline) and large (FEC-encoded)
//! broadcasts in both directions.

use adnl::OverlayShortId;
use compat_test::{skip_if_no_cpp, test_helpers::RustTestNode, CppTestNode};
use std::{sync::Arc, thread::sleep, time::Duration};

/// Port base for this test file (each test offsets by 10)
const PORT_BASE: u16 = 15100;

/// Set up a Rust + C++ node pair on an overlay.
/// Uses private overlay on C++ side (no DHT required) and public overlay on Rust side.
/// Returns (cpp_node, rust_node, overlay_short_id, cpp_overlay_id_hex)
fn setup_overlay_pair(
    port_offset: u16,
) -> (CppTestNode, RustTestNode, Arc<OverlayShortId>, String) {
    let cpp_port = PORT_BASE + port_offset;
    let rust_port = PORT_BASE + port_offset + 1;

    // 1. Spawn C++ node
    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");

    // 2. Create Rust node
    let rust_node = RustTestNode::new("127.0.0.1", rust_port, false);

    // 3. Compute overlay ID on Rust side (workchain=0, shard=-9223372036854775808 i.e. 0x8000000000000000)
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);

    // 4. Create overlay on both sides
    // Use public overlay on Rust, private overlay on C++ (C++ public overlay requires DHT)
    rust_node.add_public_overlay(&overlay_short_id);

    let rust_adnl_id = rust_node.adnl_id_hex();
    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_adnl_id])
        .expect("create C++ overlay");

    // Verify overlay IDs match
    let rust_overlay_hex =
        overlay_short_id.data().iter().map(|b| format!("{:02x}", b)).collect::<String>();
    assert_eq!(
        cpp_overlay_id.to_lowercase(),
        rust_overlay_hex.to_lowercase(),
        "Overlay IDs should match between C++ and Rust"
    );

    // 5. Exchange ADNL peers AND add to overlay neighbours
    // Rust -> C++: add C++ as ADNL peer and to overlay's known_peers/neighbours
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);

    // C++ -> Rust: add Rust as ADNL peer
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add_peer");

    (cpp, rust_node, overlay_short_id, cpp_overlay_id)
}

/// Test small broadcast from Rust to C++
#[test]
fn test_broadcast_rust_to_cpp() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_overlay_pair(10);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send a small broadcast from Rust
    let test_data = b"Hello from Rust broadcast!";
    rust_node.send_broadcast(&overlay_id, test_data);

    // Wait for C++ to receive it
    sleep(Duration::from_secs(2));

    let received = cpp.get_received_broadcasts(&cpp_overlay_id).expect("get broadcasts");

    println!("C++ received {} broadcasts after Rust->C++ send", received.len());

    // Broadcast MUST be delivered for the test to pass
    assert!(
        !received.is_empty(),
        "Rust->C++ broadcast was not delivered. Expected at least 1 broadcast."
    );
    assert_eq!(received[0].size, test_data.len(), "Broadcast size mismatch");
    println!("Rust->C++ broadcast delivered successfully!");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test small broadcast from C++ to Rust
#[test]
fn test_broadcast_cpp_to_rust() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_overlay_pair(20);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send a small broadcast from C++
    let test_data = b"Hello from C++ broadcast!";
    cpp.send_broadcast(&cpp_overlay_id, test_data, false).expect("C++ send broadcast");

    // Wait for Rust to receive it
    let received = rust_node.wait_for_broadcast(&overlay_id, 3);

    // Broadcast MUST be delivered for the test to pass
    assert!(received.is_some(), "C++->Rust broadcast was not delivered within timeout");
    let data = received.unwrap();
    assert_eq!(data, test_data, "Broadcast data mismatch");
    println!("C++->Rust broadcast delivered successfully!");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test FEC broadcast from Rust to C++ (large data, triggers FEC encoding at >768 bytes)
#[test]
fn test_fec_broadcast_rust_to_cpp() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_overlay_pair(30);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send a large broadcast (triggers FEC path, > 768 bytes)
    let test_data: Vec<u8> = (0..2000).map(|i| (i % 256) as u8).collect();
    rust_node.send_broadcast(&overlay_id, &test_data);

    // Wait for C++ to receive it
    sleep(Duration::from_secs(3));

    let received = cpp.get_received_broadcasts(&cpp_overlay_id).expect("get broadcasts");

    println!("C++ received {} FEC broadcasts after Rust->C++ send", received.len());

    // FEC broadcast MUST be delivered for the test to pass
    assert!(
        !received.is_empty(),
        "Rust->C++ FEC broadcast was not delivered. Expected at least 1 broadcast."
    );
    assert_eq!(received[0].size, test_data.len(), "FEC broadcast size mismatch");
    println!("Rust->C++ FEC broadcast delivered successfully!");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test FEC broadcast from C++ to Rust (large data, triggers FEC encoding at >768 bytes)
#[test]
fn test_fec_broadcast_cpp_to_rust() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_overlay_pair(40);

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Send a large broadcast from C++ (triggers FEC path, > 768 bytes)
    let test_data: Vec<u8> = (0..2000).map(|i| (i % 256) as u8).collect();
    cpp.send_broadcast(&cpp_overlay_id, &test_data, true).expect("C++ send FEC broadcast");

    // Wait for Rust to receive it
    let received = rust_node.wait_for_broadcast(&overlay_id, 5);

    // FEC broadcast MUST be delivered for the test to pass
    assert!(received.is_some(), "C++->Rust FEC broadcast was not delivered within timeout");
    let data = received.unwrap();
    assert_eq!(data, test_data, "FEC broadcast data mismatch");
    println!("C++->Rust FEC broadcast delivered successfully!");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}
