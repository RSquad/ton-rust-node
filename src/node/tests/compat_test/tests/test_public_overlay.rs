/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Overlay query compatibility tests between Rust and C++ implementations.
//!
//! Tests that overlay queries (request/response) work correctly between
//! Rust and C++ nodes in both directions.

use adnl::{common::TaggedTlObject, OverlayShortId};
use compat_test::{skip_if_no_cpp, test_helpers::RustTestNode, CppTestNode};
use std::{sync::Arc, thread::sleep, time::Duration};
use ton_api::{serialize_boxed, ton::rpc::adnl::Ping as AdnlPing, AnyBoxedSerialize};

/// Port base for this test file (each test offsets by 10)
const PORT_BASE: u16 = 15150;

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
    rust_node.add_cpp_peer_to_overlay(&mut cpp, &overlay_short_id);

    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add_peer");

    (cpp, rust_node, overlay_short_id, cpp_overlay_id)
}

/// Deterministic positive query test: C++ sends a valid TL query and Rust echoes it back.
#[test]
fn test_query_cpp_to_rust_echo_roundtrip() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_overlay_pair(0);

    // Add echo consumer on Rust side
    let echo = compat_test::test_helpers::EchoConsumer::new();
    rust_node.overlay.add_consumer(&overlay_id, echo).expect("add consumer");

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // C++ sends a valid TL-serialized query to Rust
    let rust_adnl_id = rust_node.adnl_id_hex();
    let query = AdnlPing { value: 0x1122_3344_5566_7788 };
    let query_bytes = serialize_boxed(&query).expect("serialize query");

    let answer = cpp
        .send_query(&cpp_overlay_id, &rust_adnl_id, &query_bytes, 5000)
        .expect("C++->Rust query should succeed");

    assert_eq!(answer, query_bytes, "C++->Rust echo reply mismatch");
    println!("C++->Rust query echo roundtrip succeeded");

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Deterministic negative query test: C++ rejects Rust query.
/// Note: C++ ADNL drops errors server-side (logs them but sends no response),
/// so the Rust side sees a timeout (Ok(None)) rather than an explicit error.
#[test]
fn test_query_rust_to_cpp_rejects_with_error() {
    skip_if_no_cpp!();

    let (mut cpp, rust_node, overlay_id, cpp_overlay_id) = setup_overlay_pair(10);

    // Configure C++ side to explicitly reject queries.
    cpp.set_query_handler(&cpp_overlay_id, "reject").expect("set reject handler");

    // Allow time for ADNL channel establishment
    sleep(Duration::from_millis(500));

    // Rust sends query to C++
    let cpp_key_id = RustTestNode::cpp_key_id(&cpp);

    // Build a valid TL query object
    let query_data = AdnlPing { value: 0x0102_0304_0506_0708 };
    let tagged: TaggedTlObject = query_data.into_tl_object().into();

    let result = rust_node.rt.block_on(async {
        rust_node.overlay.query(&cpp_key_id, &tagged, &overlay_id, Some(5000)).await
    });

    // C++ ADNL drops rejected queries without sending a response back to the peer,
    // so the Rust side either times out (Ok(None)) or gets a transport-level error.
    match result {
        Ok(None) => {
            println!("Rust->C++ query timed out as expected (C++ dropped the rejected query)");
        }
        Err(e) => {
            println!("Rust->C++ query failed with error (expected): {}", e);
        }
        Ok(Some(_)) => panic!("Expected timeout or error from C++ reject mode, got a response"),
    }

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}
