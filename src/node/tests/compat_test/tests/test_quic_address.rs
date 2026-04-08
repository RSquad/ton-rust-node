/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! QUIC address (adnl.address.quic) compatibility tests.
//!
//! Verifies that Rust and C++ nodes can establish QUIC connections when the
//! peer's QUIC address is discovered via `adnl.address.quic` in the address
//! list, rather than derived from the ADNL UDP port + 1000 offset.
//!
//! This tests the changes from C++ PR ton-blockchain/ton#2184
//! ("Store ip:port for quic in AdnlAddressList").

use adnl::common::AdnlPeers;
use compat_test::{skip_if_no_cpp, test_helpers::RustQuicTestNode, CppTestNode, TestTimeout};
use std::{thread::sleep, time::Duration};
use ton_api::{serialize_boxed, ton::ton_node::data::Data as TonNodeData, IntoBoxed};

/// Port base for QUIC address tests (unique range to avoid conflicts)
const PORT_BASE: u16 = 18300;

/// Test: Rust discovers C++ QUIC port via adnl.address.quic and sends a QUIC query.
///
/// Instead of hardcoding the QUIC port as `udp_port + 1000`, the Rust node receives
/// an AddressList containing `adnl.address.quic` with an explicit port. The QUIC
/// connection is established using that address, and a query echo roundtrip verifies
/// it works end-to-end.
#[test]
fn test_quic_query_via_address_list_rust_to_cpp() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE;
    let rust_port = PORT_BASE + 1;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    let cpp_quic_port = cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);

    // Rust adds C++ peer via an AddressList containing adnl.address.quic.
    // This exercises the new parse_quic_address → set_peer_quic_address →
    // ensure_peer_registered path (no hardcoded port offset).
    rust_node.add_cpp_peer_via_address_list(&cpp, cpp_quic_port);

    // C++ adds Rust as a peer (standard way)
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer(&rust_pubkey, "127.0.0.1", rust_port).expect("C++ add peer");

    sleep(Duration::from_millis(500));

    // Send a QUIC query from Rust to C++.
    // The query must be a valid TL-serialized object for the C++ side to process.
    let payload = b"QUIC query via adnl.address.quic";
    let tl_query = TonNodeData { data: payload.to_vec().into() };
    let query_data = serialize_boxed(&tl_query.into_boxed()).expect("serialize query TL");

    let src = rust_node.adnl_key_id();
    let dst = RustQuicTestNode::cpp_key_id(&cpp);

    println!(
        "Sending QUIC query from Rust to C++ ({} bytes), QUIC addr discovered via adnl.address.quic",
        query_data.len()
    );

    let result = rust_node.rt.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(10),
            rust_node.quic.query(
                query_data.clone(),
                Some(&*rust_node.adnl),
                &AdnlPeers::with_keys(src, dst.clone()),
                None,
            ),
        )
        .await
    });

    match result {
        Ok(Ok(Some(answer))) => {
            println!("SUCCESS: Got QUIC echo answer ({} bytes)", answer.len());
            assert_eq!(answer, query_data, "Echo data mismatch");
        }
        Ok(Ok(None)) => panic!("QUIC query returned empty answer"),
        Ok(Err(e)) => panic!("QUIC query failed: {}", e),
        Err(_) => panic!("QUIC query timed out"),
    }

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}

/// Test: C++ discovers Rust QUIC port via adnl.address.quic and sends a QUIC query.
///
/// The C++ node receives the Rust peer with an explicit `quic_port` in the
/// `add_peer` command, which includes `adnl.address.quic` in the address list.
/// C++ then sends a QUIC query to Rust using that address.
#[test]
fn test_quic_query_via_address_list_cpp_to_rust() {
    skip_if_no_cpp!();
    let _ = env_logger::try_init();
    let _timeout = TestTimeout::new(0);

    let cpp_port = PORT_BASE + 10;
    let rust_port = PORT_BASE + 11;
    let rust_quic_port = rust_port + adnl::QuicNode::OFFSET_PORT;

    let mut cpp = CppTestNode::spawn(cpp_port).expect("spawn C++");
    cpp.enable_quic().expect("enable QUIC on C++");

    let rust_node = RustQuicTestNode::new("127.0.0.1", rust_port);
    let rust_id = rust_node.adnl_id_hex();

    // Rust adds C++ as a standard peer
    rust_node.add_cpp_peer(&cpp);

    // C++ adds Rust with explicit quic_port in the address list.
    // This makes C++ include adnl.address.quic in the peer's AddressList,
    // so QuicSender::get_ip_address() uses it instead of the UDP+offset fallback.
    let rust_pubkey = rust_node.pubkey_tl_b64();
    cpp.add_peer_with_quic(&rust_pubkey, "127.0.0.1", rust_port, Some(rust_quic_port))
        .expect("C++ add peer with quic");

    sleep(Duration::from_millis(500));

    // C++ sends QUIC query to Rust
    let payload = b"QUIC query via adnl.address.quic from C++";
    let tl_query = TonNodeData { data: payload.to_vec().into() };
    let query_data = serialize_boxed(&tl_query.into_boxed()).expect("serialize query TL");

    println!(
        "C++ sending QUIC query to Rust ({} bytes), QUIC addr via adnl.address.quic",
        query_data.len()
    );

    let result = cpp.send_quic_query(&rust_id, &query_data, 5000);

    match result {
        Ok(answer) => {
            println!("SUCCESS: C++ got QUIC echo answer ({} bytes)", answer.len());
            assert_eq!(answer, query_data, "Echo data mismatch");
        }
        Err(e) => panic!("C++→Rust QUIC query failed: {}", e),
    }

    rust_node.stop();
    cpp.shutdown().expect("shutdown");
}
