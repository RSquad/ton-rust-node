/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Overlay-level QUIC compatibility tests between Rust and C++ implementations.
//!
//! Tests overlay operations where at least one direction uses QUIC transport.
//! The C++ QuicSender routes messages through ADNL to the overlay layer,
//! so overlay operations should work if the QUIC transport layer is compatible.
//!
//! Note: The overlay itself does not directly use QUIC. Instead:
//! - C++ sends via QuicSender.send_message() → QUIC → receiver's QuicSender
//!   → receiver's ADNL.receive_message() → overlay callback
//! - Rust sends via QuicTransport.send_message() → QUIC stream → C++ QuicSender
//!   → ADNL.receive_message() → overlay callback

use compat_test::{skip_if_no_cpp, test_helpers::RustQuicTestNode, CppTestNode, TestTimeout};
use std::{thread::sleep, time::Duration};
use ton_api::{serialize_boxed, ton::overlay::message::Message as OverlayMessage, IntoBoxed};
use ton_block::UInt256;

/// Port base for QUIC overlay tests
const PORT_BASE: u16 = 18100;

/// Test: C++ sends overlay message via QUIC to another C++ node
///
/// Baseline test: verifies C++ QUIC works between two C++ nodes.
/// Both nodes enable QUIC and exchange messages.
#[test]
fn test_quic_overlay_cpp_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp1_port = PORT_BASE;
    let cpp2_port = PORT_BASE + 1;

    let mut cpp1 = CppTestNode::spawn(cpp1_port).expect("spawn C++ node 1");
    let mut cpp2 = CppTestNode::spawn(cpp2_port).expect("spawn C++ node 2");

    cpp1.enable_quic().expect("enable QUIC on C++ node 1");
    cpp2.enable_quic().expect("enable QUIC on C++ node 2");

    let cpp1_id = cpp1.adnl_id().to_string();
    let cpp2_id = cpp2.adnl_id().to_string();
    println!("C++ node 1 ADNL ID: {}", cpp1_id);
    println!("C++ node 2 ADNL ID: {}", cpp2_id);

    // Create overlay on both nodes
    let overlay_name = b"test_quic_overlay_cpp2cpp";
    let overlay_id_1 = cpp1
        .create_private_overlay(overlay_name, vec![cpp1_id.clone(), cpp2_id.clone()])
        .expect("C++ 1 create private overlay");
    let overlay_id_2 = cpp2
        .create_private_overlay(overlay_name, vec![cpp1_id.clone(), cpp2_id.clone()])
        .expect("C++ 2 create private overlay");
    assert_eq!(overlay_id_1, overlay_id_2, "Overlay IDs should match");

    // Exchange ADNL peers (needed for QuicSender address resolution)
    let cpp1_pubkey = cpp1.pubkey().to_string();
    let cpp2_pubkey = cpp2.pubkey().to_string();
    cpp1.add_peer(&cpp2_pubkey, "127.0.0.1", cpp2_port).expect("C++ 1 add peer");
    cpp2.add_peer(&cpp1_pubkey, "127.0.0.1", cpp1_port).expect("C++ 2 add peer");

    sleep(Duration::from_millis(500));

    // C++ node 1 sends QUIC message to C++ node 2.
    // Data must be prefixed with overlay.message TL for the receiver's ADNL
    // to route it to the overlay callback.
    let test_data = b"C++ to C++ overlay via QUIC";
    let overlay_bytes = hex::decode(&overlay_id_1).expect("decode overlay hex");
    let mut overlay_msg = serialize_boxed(
        &OverlayMessage { overlay: UInt256::with_array(overlay_bytes.try_into().unwrap()) }
            .into_boxed(),
    )
    .expect("serialize overlay message prefix");
    overlay_msg.extend_from_slice(test_data);

    println!("C++ 1 sending QUIC message to C++ 2 ({} bytes)", overlay_msg.len());
    cpp1.send_quic_message(&cpp2_id, &overlay_msg).expect("C++ 1 send QUIC message");

    sleep(Duration::from_secs(2));

    // Check if C++ node 2 received (via ADNL → overlay callback)
    let received = cpp2.get_received_messages(&overlay_id_2).expect("get messages");
    println!("C++ node 2 received {} messages", received.len());

    assert!(
        !received.is_empty(),
        "C++→C++ QUIC overlay message not received: QuicSender may not match overlay expectations"
    );
    println!("SUCCESS: C++→C++ QUIC overlay message delivered");

    cpp1.shutdown().expect("shutdown node 1");
    cpp2.shutdown().expect("shutdown node 2");
}

/// Test: Rust sends overlay message via both UDP and QUIC, C++ receives both
///
/// First sends a baseline UDP overlay message to confirm overlay routing works,
/// then sends via QUIC transport and verifies C++ receives it through the
/// ADNL → overlay callback path.
#[test]
fn test_quic_overlay_message_rust_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE + 10;
    let rust_port = PORT_BASE + 11;

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

    // Exchange peers (ADNL + QUIC)
    rust_node.add_cpp_peer_full(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_secs(1));

    // Send overlay message from Rust via regular ADNL (as baseline)
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    let baseline_data = b"Baseline: Rust overlay message via UDP";
    println!("Sending baseline overlay message via UDP ({} bytes)", baseline_data.len());
    rust_node.send_overlay_message(&overlay_short_id, &cpp_key_id, baseline_data);

    sleep(Duration::from_secs(2));

    let baseline_received =
        cpp.get_received_messages(&cpp_overlay_id).expect("get baseline messages");
    println!("Baseline (UDP): C++ received {} overlay messages", baseline_received.len());

    // Clear for QUIC test
    cpp.clear_received_messages(&cpp_overlay_id).expect("clear messages");

    // Now send via QUIC transport with overlay TL wrapping
    let quic_data = b"QUIC: Rust to C++ overlay test";
    println!("Sending QUIC overlay message from Rust to C++ ({} bytes)", quic_data.len());
    rust_node.send_quic_overlay_message(&cpp_key_id, &overlay_short_id, quic_data);

    sleep(Duration::from_secs(2));

    let quic_received = cpp.get_received_messages(&cpp_overlay_id).expect("get QUIC messages");
    println!("QUIC transport: C++ received {} messages via overlay", quic_received.len());

    assert!(!baseline_received.is_empty(), "Baseline UDP overlay message not received");
    assert!(
        !quic_received.is_empty(),
        "UDP overlay messages work but QUIC messages don't reach overlay"
    );
    println!("SUCCESS: Both UDP and QUIC messages delivered to C++ overlay");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: C++ sends QUIC message to Rust, Rust receives via QUIC transport
///
/// Verifies raw QUIC message delivery from C++ to Rust. The message arrives
/// at the QuicTestSubscriber (transport level), not through overlay routing.
/// Expected: PASS — Rust server accepts C++ connections without SNI.
#[test]
fn test_quic_message_cpp_to_rust() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE + 20;
    let rust_port = PORT_BASE + 21;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    // Create overlay
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let _cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");

    rust_node.add_public_overlay(&overlay_short_id);

    // Exchange peers
    rust_node.add_cpp_peer_full(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_secs(1));

    // C++ sends QUIC message to Rust
    let test_data = b"C++ overlay message via QUIC to Rust";
    println!("C++ sending QUIC message to Rust ({} bytes)", test_data.len());
    let send_result = cpp.send_quic_message(&rust_id, test_data);
    println!("C++ send_quic_message: {:?}", send_result.is_ok());

    // Try to receive on Rust QUIC subscriber
    let received = rust_node.recv_quic_message(3);

    let data = received.expect("C++→Rust QUIC overlay message should be received");
    println!("SUCCESS: Rust received QUIC message from C++ ({} bytes)", data.len());

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: QUIC overlay query from Rust to C++ with echo handler
///
/// Sends a QUIC query from Rust to C++ where C++ has an echo handler set up.
/// The query goes through QuicTransport → C++ QuicSender → ADNL → overlay query handler.
#[test]
fn test_quic_overlay_query_rust_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE + 30;
    let rust_port = PORT_BASE + 31;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    let rust_id = rust_node.adnl_id_hex();
    let cpp_id = cpp.adnl_id().to_string();

    // Create overlay with echo handler
    let overlay_name_bytes = rust_node.compute_overlay_name(0, i64::MIN);
    let overlay_short_id = rust_node.compute_overlay_short_id(0, i64::MIN);

    let cpp_overlay_id = cpp
        .create_private_overlay(&overlay_name_bytes, vec![rust_id.clone(), cpp_id.clone()])
        .expect("C++ create private overlay");
    cpp.set_query_handler(&cpp_overlay_id, "echo").expect("set echo handler");

    rust_node.add_public_overlay(&overlay_short_id);

    // Exchange peers
    rust_node.add_cpp_peer_full(&mut cpp, &overlay_short_id);
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_secs(1));

    // First verify baseline: overlay query via UDP works
    let cpp_key_id = RustQuicTestNode::cpp_key_id(&cpp);
    println!("Baseline: testing overlay query via UDP...");
    let baseline_result =
        cpp.send_query(&cpp_overlay_id, &rust_node.adnl_id_hex(), b"baseline query", 5000);
    println!("Baseline overlay query result: {:?}", baseline_result.is_ok());

    // Now try QUIC query with overlay wrapping
    let query_data = b"QUIC overlay query from Rust";
    println!("Sending QUIC overlay query from Rust to C++ ({} bytes)", query_data.len());

    let result = rust_node.send_quic_overlay_query(&cpp_key_id, &overlay_short_id, query_data, 10);

    let answer = result.expect("QUIC overlay query should succeed");
    println!("SUCCESS: Got QUIC overlay query answer ({} bytes)", answer.len());
    assert!(!answer.is_empty(), "QUIC overlay query answer should not be empty");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}
