/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! TwostepFec broadcast relay tests between Rust and C++ implementations.
//!
//! Tests a 6-node topology: Sender -> 4 Bridges -> Leaf
//! - Sender connected to all 4 bridges (TwostepFec requires >=4 neighbors)
//! - Each bridge connected to leaf
//! - Leaf NOT directly connected to sender
//! - Data >= 513 bytes triggers TwostepFec (if RLDP + enough neighbors)
//! - All nodes need RLDP enabled
//!
//! TwostepFec sends unique FEC parts to each neighbor via RLDP.
//! Each receiver redistributes received parts to its neighbors.
//! The leaf can only receive through redistribution from bridges.

use adnl::OverlayShortId;
use compat_test::{skip_if_no_cpp, test_helpers::RustTestNode, CppTestNode};
use std::{sync::Arc, thread::sleep, time::Duration};

/// Port base for this test file
const PORT_BASE: u16 = 15700;

/// Number of bridge nodes (must be >= 4 for TwostepFec)
const NUM_BRIDGES: usize = 4;

/// Data size for TwostepFec (>= 513 bytes)
const TWOSTEP_DATA_SIZE: usize = 2048;

fn make_test_data(size: usize, tag: u8) -> Vec<u8> {
    (0..size).map(|i| ((i % 251) as u8).wrapping_add(tag)).collect()
}

enum Node {
    Rust(RustTestNode),
    Cpp(CppTestNode),
}

struct TwostepTopology {
    sender: Node,
    bridges: Vec<Node>,
    leaf: Node,
    overlay_short_id: Arc<OverlayShortId>,
    overlay_id_hex: String,
}

impl TwostepTopology {
    fn shutdown(self) {
        match self.sender {
            Node::Rust(r) => r.stop(),
            Node::Cpp(mut c) => {
                let _ = c.shutdown();
            }
        }
        for node in self.bridges {
            match node {
                Node::Rust(r) => r.stop(),
                Node::Cpp(mut c) => {
                    let _ = c.shutdown();
                }
            }
        }
        match self.leaf {
            Node::Rust(r) => r.stop(),
            Node::Cpp(mut c) => {
                let _ = c.shutdown();
            }
        }
    }
}

/// Create a 6-node topology for TwostepFec relay testing.
///
/// `sender_is_rust`: whether sender is Rust (true) or C++ (false)
/// `bridge_rust_mask`: bitmask of which bridges are Rust (bit 0 = bridge 0, etc.)
/// `leaf_is_rust`: whether leaf is Rust (true) or C++ (false)
fn setup_twostep_topology(
    port_offset: u16,
    sender_is_rust: bool,
    bridge_rust_mask: u8,
    leaf_is_rust: bool,
) -> TwostepTopology {
    let base = PORT_BASE + port_offset;
    // Ports: sender=base, bridges=base+1..base+4, leaf=base+5, helper=base+8
    let sender_port = base;
    let bridge_ports: Vec<u16> = (0..NUM_BRIDGES).map(|i| base + 1 + i as u16).collect();
    let leaf_port = base + 1 + NUM_BRIDGES as u16;
    let helper_port = base + 8;

    // Compute overlay IDs using a helper node
    let helper = RustTestNode::new("127.0.0.1", helper_port, false);
    let overlay_short_id = helper.compute_overlay_short_id(0, i64::MIN);
    let overlay_name_bytes = helper.compute_overlay_name(0, i64::MIN);
    let overlay_id_hex =
        overlay_short_id.data().iter().map(|b| format!("{:02x}", b)).collect::<String>();
    helper.stop();

    // Create all nodes (with RLDP for Rust, twostep for C++)
    let sender_rust =
        if sender_is_rust { Some(RustTestNode::new("127.0.0.1", sender_port, true)) } else { None };
    let mut sender_cpp = if !sender_is_rust {
        Some(CppTestNode::spawn(sender_port).expect("spawn sender C++"))
    } else {
        None
    };

    let mut bridge_rusts: Vec<Option<RustTestNode>> = Vec::new();
    let mut bridge_cpps: Vec<Option<CppTestNode>> = Vec::new();
    for i in 0..NUM_BRIDGES {
        let is_rust = (bridge_rust_mask >> i) & 1 == 1;
        if is_rust {
            bridge_rusts.push(Some(RustTestNode::new("127.0.0.1", bridge_ports[i], true)));
            bridge_cpps.push(None);
        } else {
            bridge_rusts.push(None);
            bridge_cpps.push(Some(CppTestNode::spawn(bridge_ports[i]).expect("spawn bridge C++")));
        }
    }

    let leaf_rust =
        if leaf_is_rust { Some(RustTestNode::new("127.0.0.1", leaf_port, true)) } else { None };
    let mut leaf_cpp = if !leaf_is_rust {
        Some(CppTestNode::spawn(leaf_port).expect("spawn leaf C++"))
    } else {
        None
    };

    // Collect ADNL IDs
    let sender_id = match (&sender_rust, &sender_cpp) {
        (Some(r), _) => r.adnl_id_hex(),
        (_, Some(c)) => c.adnl_id().to_string(),
        _ => unreachable!(),
    };
    let bridge_ids: Vec<String> = (0..NUM_BRIDGES)
        .map(|i| match (&bridge_rusts[i], &bridge_cpps[i]) {
            (Some(r), _) => r.adnl_id_hex(),
            (_, Some(c)) => c.adnl_id().to_string(),
            _ => unreachable!(),
        })
        .collect();
    let leaf_id = match (&leaf_rust, &leaf_cpp) {
        (Some(r), _) => r.adnl_id_hex(),
        (_, Some(c)) => c.adnl_id().to_string(),
        _ => unreachable!(),
    };

    // Create overlays
    // Sender: neighbors are all 4 bridges
    if let Some(ref r) = sender_rust {
        r.add_public_overlay(&overlay_short_id);
    }
    if let Some(ref mut c) = sender_cpp {
        c.create_private_overlay_twostep(&overlay_name_bytes, bridge_ids.clone())
            .expect("sender C++ create overlay");
    }

    // Each bridge: neighbors are sender + leaf + other bridges
    for i in 0..NUM_BRIDGES {
        let mut bridge_peers = vec![sender_id.clone(), leaf_id.clone()];
        for j in 0..NUM_BRIDGES {
            if i != j {
                bridge_peers.push(bridge_ids[j].clone());
            }
        }
        if let Some(ref r) = bridge_rusts[i] {
            r.add_public_overlay(&overlay_short_id);
        }
        if let Some(ref mut c) = bridge_cpps[i] {
            c.create_private_overlay_twostep(&overlay_name_bytes, bridge_peers)
                .expect("bridge C++ create overlay");
        }
    }

    // Leaf: neighbors are all 4 bridges (NOT sender)
    if let Some(ref r) = leaf_rust {
        r.add_public_overlay(&overlay_short_id);
    }
    if let Some(ref mut c) = leaf_cpp {
        c.create_private_overlay_twostep(&overlay_name_bytes, bridge_ids.clone())
            .expect("leaf C++ create overlay");
    }

    // Wire ADNL peers
    // Sender <-> each bridge
    for i in 0..NUM_BRIDGES {
        wire_nodes(
            &sender_rust,
            sender_cpp.as_mut(),
            &bridge_rusts[i],
            bridge_cpps[i].as_mut(),
            &overlay_short_id,
        );
    }

    // Each bridge <-> leaf
    for i in 0..NUM_BRIDGES {
        wire_nodes(
            &bridge_rusts[i],
            bridge_cpps[i].as_mut(),
            &leaf_rust,
            leaf_cpp.as_mut(),
            &overlay_short_id,
        );
    }

    // Bridges <-> each other (for redistribution of FEC parts)
    for i in 0..NUM_BRIDGES {
        for j in (i + 1)..NUM_BRIDGES {
            wire_nodes_by_idx(&bridge_rusts, &mut bridge_cpps, i, j, &overlay_short_id);
        }
    }

    // Package nodes
    let sender =
        if let Some(r) = sender_rust { Node::Rust(r) } else { Node::Cpp(sender_cpp.unwrap()) };

    let bridges: Vec<Node> = (0..NUM_BRIDGES)
        .map(|i| {
            if let Some(r) = bridge_rusts[i].take() {
                Node::Rust(r)
            } else {
                Node::Cpp(bridge_cpps[i].take().unwrap())
            }
        })
        .collect();

    let leaf = if let Some(r) = leaf_rust { Node::Rust(r) } else { Node::Cpp(leaf_cpp.unwrap()) };

    TwostepTopology { sender, bridges, leaf, overlay_short_id, overlay_id_hex }
}

/// Wire two nodes as ADNL peers and overlay neighbors (bidirectional).
fn wire_nodes(
    a_rust: &Option<RustTestNode>,
    a_cpp: Option<&mut CppTestNode>,
    b_rust: &Option<RustTestNode>,
    b_cpp: Option<&mut CppTestNode>,
    overlay_id: &Arc<OverlayShortId>,
) {
    match (a_rust.as_ref(), a_cpp, b_rust.as_ref(), b_cpp) {
        (Some(a), _, Some(b), _) => {
            a.add_rust_peer_to_overlay(b, overlay_id);
            b.add_rust_peer_to_overlay(a, overlay_id);
        }
        (Some(a), _, _, Some(b)) => {
            a.add_cpp_peer_to_overlay(b, overlay_id);
            b.add_peer(&a.pubkey_tl_b64(), "127.0.0.1", a.port).expect("C++ add_peer");
        }
        (_, Some(a), Some(b), _) => {
            b.add_cpp_peer_to_overlay(a, overlay_id);
            a.add_peer(&b.pubkey_tl_b64(), "127.0.0.1", b.port).expect("C++ add_peer");
        }
        (_, Some(a), _, Some(b)) => {
            let b_pubkey = b.pubkey().to_string();
            let b_port = b.udp_port();
            a.add_peer(&b_pubkey, "127.0.0.1", b_port).expect("C++ add_peer a->b");
            let a_pubkey = a.pubkey().to_string();
            let a_port = a.udp_port();
            b.add_peer(&a_pubkey, "127.0.0.1", a_port).expect("C++ add_peer b->a");
        }
        _ => unreachable!(),
    }
}

/// Wire two bridge nodes by index from the bridge arrays.
fn wire_nodes_by_idx(
    rusts: &[Option<RustTestNode>],
    cpps: &mut [Option<CppTestNode>],
    i: usize,
    j: usize,
    overlay_id: &Arc<OverlayShortId>,
) {
    // Can't borrow two elements mutably at once, so handle it carefully
    match (rusts[i].as_ref(), rusts[j].as_ref()) {
        (Some(a), Some(b)) => {
            a.add_rust_peer_to_overlay(b, overlay_id);
            b.add_rust_peer_to_overlay(a, overlay_id);
        }
        (Some(a), None) => {
            let b = cpps[j].as_mut().unwrap();
            a.add_cpp_peer_to_overlay(b, overlay_id);
            b.add_peer(&a.pubkey_tl_b64(), "127.0.0.1", a.port).expect("C++ add_peer");
        }
        (None, Some(b)) => {
            let a = cpps[i].as_mut().unwrap();
            b.add_cpp_peer_to_overlay(a, overlay_id);
            a.add_peer(&b.pubkey_tl_b64(), "127.0.0.1", b.port).expect("C++ add_peer");
        }
        (None, None) => {
            // Both C++ - split borrow by using pointers
            let (left, right) = cpps.split_at_mut(j);
            let a = left[i].as_mut().unwrap();
            let b = right[0].as_mut().unwrap();
            let b_pubkey = b.pubkey().to_string();
            let b_port = b.udp_port();
            a.add_peer(&b_pubkey, "127.0.0.1", b_port).expect("C++ add_peer");
            let a_pubkey = a.pubkey().to_string();
            let a_port = a.udp_port();
            b.add_peer(&a_pubkey, "127.0.0.1", a_port).expect("C++ add_peer");
        }
    }
}

// =================== Tests ===================

/// Test A: Rust sender, all Rust bridges, C++ leaf
/// Verifies Rust TwostepFec reaches C++ through Rust redistribution
#[test]
fn test_twostep_rust_sender_cpp_leaf() {
    skip_if_no_cpp!();

    let mut topo = setup_twostep_topology(0, true, 0b1111, false);
    sleep(Duration::from_millis(500));

    let test_data = make_test_data(TWOSTEP_DATA_SIZE, 0xA1);

    // Send TwostepFec from Rust sender
    if let Node::Rust(ref sender) = topo.sender {
        sender.send_broadcast_two_step(&topo.overlay_short_id, &test_data);
    }

    // Wait for redistribution and delivery
    sleep(Duration::from_secs(5));

    // Check C++ leaf received the broadcast
    if let Node::Cpp(ref mut leaf) = topo.leaf {
        let received = leaf.get_received_broadcasts(&topo.overlay_id_hex).expect("get broadcasts");
        assert!(!received.is_empty(), "C++ leaf did not get TwostepFec broadcast via Rust bridges");
        assert_eq!(received[0].size, test_data.len(), "Broadcast size mismatch");
        println!("test_twostep_rust_sender_cpp_leaf: PASSED");
    }

    topo.shutdown();
}

/// Test B: C++ sender, all C++ bridges, Rust leaf
/// Verifies C++ TwostepFec reaches Rust through C++ redistribution
#[test]
fn test_twostep_cpp_sender_rust_leaf() {
    skip_if_no_cpp!();

    let mut topo = setup_twostep_topology(10, false, 0b0000, true);
    sleep(Duration::from_millis(500));

    let test_data = make_test_data(TWOSTEP_DATA_SIZE, 0xB2);

    // Send FEC broadcast from C++ sender (C++ will use twostep if enabled)
    if let Node::Cpp(ref mut sender) = topo.sender {
        sender.send_broadcast(&topo.overlay_id_hex, &test_data, true).expect("C++ send broadcast");
    }

    // Check Rust leaf - call wait_for_broadcast immediately to avoid BroadcastReceiver drop
    if let Node::Rust(ref leaf) = topo.leaf {
        let received = leaf.wait_for_broadcast(&topo.overlay_short_id, 10);
        assert!(received.is_some(), "Rust leaf did not get TwostepFec broadcast via C++ bridges");
        assert_eq!(received.unwrap(), test_data, "Broadcast data mismatch");
        println!("test_twostep_cpp_sender_rust_leaf: PASSED");
    }

    topo.shutdown();
}

/// Test C: Rust sender, mixed bridges (2 Rust, 2 C++), Rust leaf
/// Verifies mixed Rust/C++ redistribution works
#[test]
fn test_twostep_mixed_bridges_rust_leaf() {
    skip_if_no_cpp!();

    // Bridges 0,1 are Rust, 2,3 are C++
    let topo = setup_twostep_topology(20, true, 0b0011, true);
    sleep(Duration::from_millis(500));

    let test_data = make_test_data(TWOSTEP_DATA_SIZE, 0xC3);

    // Send TwostepFec from Rust sender
    if let Node::Rust(ref sender) = topo.sender {
        sender.send_broadcast_two_step(&topo.overlay_short_id, &test_data);
    }

    // Check Rust leaf
    if let Node::Rust(ref leaf) = topo.leaf {
        let received = leaf.wait_for_broadcast(&topo.overlay_short_id, 10);
        assert!(received.is_some(), "Rust leaf did not get TwostepFec broadcast via mixed bridges");
        assert_eq!(received.unwrap(), test_data, "Broadcast data mismatch");
        println!("test_twostep_mixed_bridges_rust_leaf: PASSED");
    }

    topo.shutdown();
}

/// Test D: C++ sender, mixed bridges (2 Rust, 2 C++), C++ leaf
/// Verifies C++ TwostepFec works with mixed redistribution to C++ leaf
#[test]
fn test_twostep_mixed_bridges_cpp_leaf() {
    skip_if_no_cpp!();

    // Bridges 0,1 are Rust, 2,3 are C++
    let mut topo = setup_twostep_topology(30, false, 0b0011, false);
    sleep(Duration::from_millis(500));

    let test_data = make_test_data(TWOSTEP_DATA_SIZE, 0xD4);

    // Send FEC broadcast from C++ sender
    if let Node::Cpp(ref mut sender) = topo.sender {
        sender.send_broadcast(&topo.overlay_id_hex, &test_data, true).expect("C++ send broadcast");
    }

    // Wait for redistribution and delivery
    sleep(Duration::from_secs(5));

    // Check C++ leaf received the broadcast
    if let Node::Cpp(ref mut leaf) = topo.leaf {
        let received = leaf.get_received_broadcasts(&topo.overlay_id_hex).expect("get broadcasts");
        assert!(
            !received.is_empty(),
            "C++ leaf did not get TwostepFec broadcast via mixed bridges"
        );
        assert_eq!(received[0].size, test_data.len(), "Broadcast size mismatch");
        println!("test_twostep_mixed_bridges_cpp_leaf: PASSED");
    }

    topo.shutdown();
}
