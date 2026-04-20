/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! FEC broadcast relay tests between Rust and C++ implementations.
//!
//! Tests a 3-node linear topology: Sender -> Relay -> Receiver
//! where Sender and Receiver are NOT directly connected.
//! A large broadcast (>768 bytes) triggers FEC encoding.
//! The relay node must receive, reassemble, and redistribute the broadcast
//! to the receiver.
//!
//! Test matrix:
//! | Test | Sender | Relay | Receiver |
//! |------|--------|-------|----------|
//! | 1    | Rust   | C++   | Rust     |
//! | 2    | C++    | Rust  | C++      |
//! | 3    | Rust   | Rust  | C++      |
//! | 4    | C++    | C++   | Rust     |

use adnl::OverlayShortId;
use compat_test::{skip_if_no_cpp, test_helpers::RustTestNode, CppTestNode};
use std::{sync::Arc, thread::sleep, time::Duration};

/// Port base for this test file (each test offsets by 10)
const PORT_BASE: u16 = 15600;

/// FEC broadcast data size (must be > 768 to trigger FEC)
const FEC_DATA_SIZE: usize = 2000;

/// Generate test data of given size with a tag byte for identification
fn make_test_data(size: usize, tag: u8) -> Vec<u8> {
    (0..size).map(|i| ((i % 251) as u8).wrapping_add(tag)).collect()
}

/// Enum to track node role in the topology
enum Node {
    Rust(RustTestNode),
    Cpp(CppTestNode),
}

/// Setup result containing the 3 nodes plus overlay info
struct RelayTopology {
    sender: Node,
    relay: Node,
    receiver: Node,
    /// Overlay short ID (for Rust nodes)
    overlay_short_id: Arc<OverlayShortId>,
    /// Overlay ID hex string (for C++ nodes)
    overlay_id_hex: String,
    /// Overlay name bytes (for creating overlays)
    _overlay_name: Vec<u8>,
}

impl RelayTopology {
    fn shutdown(self) {
        match self.sender {
            Node::Rust(r) => r.stop(),
            Node::Cpp(mut c) => {
                let _ = c.shutdown();
            }
        }
        match self.relay {
            Node::Rust(r) => r.stop(),
            Node::Cpp(mut c) => {
                let _ = c.shutdown();
            }
        }
        match self.receiver {
            Node::Rust(r) => r.stop(),
            Node::Cpp(mut c) => {
                let _ = c.shutdown();
            }
        }
    }
}

/// Create a 3-node relay topology.
///
/// `roles` is (sender_is_rust, relay_is_rust, receiver_is_rust).
/// Wiring: sender <-> relay, relay <-> receiver. NOT sender <-> receiver.
fn setup_relay_topology(
    port_offset: u16,
    sender_is_rust: bool,
    relay_is_rust: bool,
    receiver_is_rust: bool,
) -> RelayTopology {
    let port0 = PORT_BASE + port_offset; // sender
    let port1 = PORT_BASE + port_offset + 1; // relay
    let port2 = PORT_BASE + port_offset + 2; // receiver

    // First, create a temporary Rust node just to compute overlay IDs consistently
    // (we'll reuse it if sender is Rust, otherwise stop it)
    let helper = RustTestNode::new("127.0.0.1", port0 + 5, false);
    let overlay_short_id = helper.compute_overlay_short_id(0, i64::MIN);
    let overlay_name_bytes = helper.compute_overlay_name(0, i64::MIN);
    let overlay_id_hex =
        overlay_short_id.data().iter().map(|b| format!("{:02x}", b)).collect::<String>();
    helper.stop();

    // Create all nodes
    let sender_rust =
        if sender_is_rust { Some(RustTestNode::new("127.0.0.1", port0, false)) } else { None };
    let mut sender_cpp = if !sender_is_rust {
        Some(CppTestNode::spawn(port0).expect("spawn sender C++"))
    } else {
        None
    };

    let relay_rust =
        if relay_is_rust { Some(RustTestNode::new("127.0.0.1", port1, false)) } else { None };
    let mut relay_cpp = if !relay_is_rust {
        Some(CppTestNode::spawn(port1).expect("spawn relay C++"))
    } else {
        None
    };

    let receiver_rust =
        if receiver_is_rust { Some(RustTestNode::new("127.0.0.1", port2, false)) } else { None };
    let mut receiver_cpp = if !receiver_is_rust {
        Some(CppTestNode::spawn(port2).expect("spawn receiver C++"))
    } else {
        None
    };

    // Collect all ADNL IDs for C++ private overlay creation
    let sender_adnl_id = match (&sender_rust, &sender_cpp) {
        (Some(r), _) => r.adnl_id_hex(),
        (_, Some(c)) => c.adnl_id().to_string(),
        _ => unreachable!(),
    };
    let relay_adnl_id = match (&relay_rust, &relay_cpp) {
        (Some(r), _) => r.adnl_id_hex(),
        (_, Some(c)) => c.adnl_id().to_string(),
        _ => unreachable!(),
    };
    let receiver_adnl_id = match (&receiver_rust, &receiver_cpp) {
        (Some(r), _) => r.adnl_id_hex(),
        (_, Some(c)) => c.adnl_id().to_string(),
        _ => unreachable!(),
    };

    // Create overlays on all nodes
    // Rust nodes use public overlay; C++ nodes use private overlay with their direct neighbors
    if let Some(ref r) = sender_rust {
        r.add_public_overlay(&overlay_short_id);
    }
    if let Some(ref mut c) = sender_cpp {
        // Sender's only neighbor is relay
        c.create_private_overlay(&overlay_name_bytes, vec![relay_adnl_id.clone()])
            .expect("sender C++ create overlay");
    }

    if let Some(ref r) = relay_rust {
        r.add_public_overlay(&overlay_short_id);
    }
    if let Some(ref mut c) = relay_cpp {
        // Relay's neighbors are sender and receiver
        c.create_private_overlay(
            &overlay_name_bytes,
            vec![sender_adnl_id.clone(), receiver_adnl_id.clone()],
        )
        .expect("relay C++ create overlay");
    }

    if let Some(ref r) = receiver_rust {
        r.add_public_overlay(&overlay_short_id);
    }
    if let Some(ref mut c) = receiver_cpp {
        // Receiver's only neighbor is relay
        c.create_private_overlay(&overlay_name_bytes, vec![relay_adnl_id.clone()])
            .expect("receiver C++ create overlay");
    }

    // Wire ADNL peers: sender <-> relay, relay <-> receiver
    // NOT sender <-> receiver (that's the whole point of the relay test)

    // === sender <-> relay ===
    wire_pair(&sender_rust, &mut sender_cpp, &relay_rust, &mut relay_cpp, &overlay_short_id);

    // === relay <-> receiver ===
    wire_pair(&relay_rust, &mut relay_cpp, &receiver_rust, &mut receiver_cpp, &overlay_short_id);

    // Package nodes
    let sender =
        if let Some(r) = sender_rust { Node::Rust(r) } else { Node::Cpp(sender_cpp.unwrap()) };
    let relay =
        if let Some(r) = relay_rust { Node::Rust(r) } else { Node::Cpp(relay_cpp.unwrap()) };
    let receiver =
        if let Some(r) = receiver_rust { Node::Rust(r) } else { Node::Cpp(receiver_cpp.unwrap()) };

    RelayTopology {
        sender,
        relay,
        receiver,
        overlay_short_id,
        overlay_id_hex,
        _overlay_name: overlay_name_bytes,
    }
}

/// Wire two nodes as ADNL peers and overlay neighbors (bidirectional).
/// Handles all 4 combinations of Rust/C++ for each side.
fn wire_pair(
    a_rust: &Option<RustTestNode>,
    a_cpp: &mut Option<CppTestNode>,
    b_rust: &Option<RustTestNode>,
    b_cpp: &mut Option<CppTestNode>,
    overlay_id: &Arc<OverlayShortId>,
) {
    match (a_rust.as_ref(), a_cpp.as_mut(), b_rust.as_ref(), b_cpp.as_mut()) {
        // Both Rust
        (Some(a), _, Some(b), _) => {
            a.add_rust_peer_to_overlay(b, overlay_id);
            b.add_rust_peer_to_overlay(a, overlay_id);
        }
        // A=Rust, B=C++
        (Some(a), _, _, Some(b)) => {
            a.add_cpp_peer_to_overlay(b, overlay_id);
            b.add_peer(&a.pubkey_tl_b64(), "127.0.0.1", a.port).expect("C++ add_peer");
        }
        // A=C++, B=Rust
        (_, Some(a), Some(b), _) => {
            b.add_cpp_peer_to_overlay(a, overlay_id);
            a.add_peer(&b.pubkey_tl_b64(), "127.0.0.1", b.port).expect("C++ add_peer");
        }
        // Both C++
        (_, Some(a), _, Some(b)) => {
            // C++ private overlays handle peering automatically via the peer list
            // But we still need to add ADNL peers
            let b_pubkey = b.pubkey().to_string();
            let b_port = b.udp_port();
            a.add_peer(&b_pubkey, "127.0.0.1", b_port).expect("C++ add_peer a->b");
            let a_pubkey = a.pubkey().to_string();
            let a_port = a.udp_port();
            b.add_peer(&a_pubkey, "127.0.0.1", a_port).expect("C++ add_peer b->a");
        }
        _ => unreachable!("Invalid node combination"),
    }
}

// =================== Tests ===================

/// Test 1: Rust sender -> C++ relay -> Rust receiver
#[test]
fn test_fec_relay_rust_cpp_rust() {
    skip_if_no_cpp!();

    let topo = setup_relay_topology(0, true, false, true);
    sleep(Duration::from_secs(2));

    let test_data = make_test_data(FEC_DATA_SIZE, 0x11);

    // Send from Rust sender, then immediately wait on Rust receiver.
    // NOTE: wait_for_broadcast must be called soon after send because
    // BroadcastReceiver drops data pushed before pop() is first called.
    if let Node::Rust(ref sender) = topo.sender {
        sender.send_broadcast(&topo.overlay_short_id, &test_data);
    }

    if let Node::Rust(ref receiver) = topo.receiver {
        let received = receiver.wait_for_broadcast(&topo.overlay_short_id, 20);
        assert!(received.is_some(), "Rust receiver did not get FEC broadcast via C++ relay");
        assert_eq!(received.unwrap(), test_data, "Broadcast data mismatch");
        println!("test_fec_relay_rust_cpp_rust: PASSED");
    }

    topo.shutdown();
}

/// Test 2: C++ sender -> Rust relay -> C++ receiver
#[test]
fn test_fec_relay_cpp_rust_cpp() {
    skip_if_no_cpp!();

    let mut topo = setup_relay_topology(10, false, true, false);
    sleep(Duration::from_secs(2));

    let test_data = make_test_data(FEC_DATA_SIZE, 0x22);

    // Send from C++ sender
    if let Node::Cpp(ref mut sender) = topo.sender {
        sender.send_broadcast(&topo.overlay_id_hex, &test_data, true).expect("C++ send broadcast");
    }

    // Wait for relay and redistribution
    sleep(Duration::from_secs(12));

    // Check C++ receiver got the broadcast
    if let Node::Cpp(ref mut receiver) = topo.receiver {
        let received =
            receiver.get_received_broadcasts(&topo.overlay_id_hex).expect("get broadcasts");
        assert!(!received.is_empty(), "C++ receiver did not get FEC broadcast via Rust relay");
        assert_eq!(received[0].size, test_data.len(), "Broadcast size mismatch");
        println!("test_fec_relay_cpp_rust_cpp: PASSED");
    }

    topo.shutdown();
}

/// Test 3: Rust sender -> Rust relay -> C++ receiver
#[test]
fn test_fec_relay_rust_rust_cpp() {
    skip_if_no_cpp!();

    let mut topo = setup_relay_topology(20, true, true, false);
    sleep(Duration::from_secs(2));

    let test_data = make_test_data(FEC_DATA_SIZE, 0x33);

    // Send from Rust sender
    if let Node::Rust(ref sender) = topo.sender {
        sender.send_broadcast(&topo.overlay_short_id, &test_data);
    }

    // Wait for relay and redistribution
    sleep(Duration::from_secs(12));

    // Check C++ receiver got the broadcast
    if let Node::Cpp(ref mut receiver) = topo.receiver {
        let received =
            receiver.get_received_broadcasts(&topo.overlay_id_hex).expect("get broadcasts");
        assert!(!received.is_empty(), "C++ receiver did not get FEC broadcast via Rust relay");
        assert_eq!(received[0].size, test_data.len(), "Broadcast size mismatch");
        println!("test_fec_relay_rust_rust_cpp: PASSED");
    }

    topo.shutdown();
}

/// Test 4: C++ sender -> C++ relay -> Rust receiver
#[test]
fn test_fec_relay_cpp_cpp_rust() {
    skip_if_no_cpp!();

    let mut topo = setup_relay_topology(30, false, false, true);
    sleep(Duration::from_secs(2));

    let test_data = make_test_data(FEC_DATA_SIZE, 0x44);

    // Send from C++ sender, then immediately wait on Rust receiver.
    if let Node::Cpp(ref mut sender) = topo.sender {
        sender.send_broadcast(&topo.overlay_id_hex, &test_data, true).expect("C++ send broadcast");
    }

    if let Node::Rust(ref receiver) = topo.receiver {
        let received = receiver.wait_for_broadcast(&topo.overlay_short_id, 20);
        assert!(received.is_some(), "Rust receiver did not get FEC broadcast via C++ relay");
        assert_eq!(received.unwrap(), test_data, "Broadcast data mismatch");
        println!("test_fec_relay_cpp_cpp_rust: PASSED");
    }

    topo.shutdown();
}
