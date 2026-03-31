/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Overlay ID Calculation Compatibility Tests
//!
//! Tests that verify both Rust and C++ implementations compute identical
//! overlay IDs from the same input.

use compat_test::{skip_if_no_cpp, CppTestNode};

/// Test that overlay ID calculation matches between Rust and C++
#[test]
fn test_overlay_id_calculation_matches() {
    skip_if_no_cpp!();

    let mut cpp_node = CppTestNode::spawn(14010).expect("Failed to spawn C++ node");

    // Test various overlay names
    let medium_name = "x".repeat(100);
    let long_name = "y".repeat(300);
    let test_names: Vec<&str> = vec![
        "test_overlay",
        "catchain",
        "validator_session",
        // Note: Empty name is not tested here as C++ rejects empty base64 input
        "a",          // short name
        &medium_name, // medium name
        &long_name,   // long name (> 254 bytes)
    ];

    for name in test_names {
        // Get C++ computed overlay ID
        let cpp_id = cpp_node
            .compute_overlay_id(name.as_bytes())
            .expect(&format!("C++ failed to compute overlay ID for '{}'", name));

        // Get Rust computed overlay ID
        let rust_id = compat_test::overlay_id::compute_overlay_id(name.as_bytes());
        let rust_id_hex = hex::encode(rust_id);

        // Compare (C++ returns uppercase hex, Rust returns lowercase)
        assert_eq!(
            cpp_id.to_lowercase(),
            rust_id_hex.to_lowercase(),
            "Overlay ID mismatch for name '{}': C++={}, Rust={}",
            name,
            cpp_id,
            rust_id_hex
        );

        println!(
            "Overlay ID matches for '{}': {}",
            if name.len() > 20 { &name[..20] } else { name },
            rust_id_hex
        );
    }

    cpp_node.shutdown().expect("Failed to shutdown C++ node");
}

/// Test overlay ID with binary data
#[test]
fn test_overlay_id_binary_data() {
    skip_if_no_cpp!();

    let mut cpp_node = CppTestNode::spawn(14011).expect("Failed to spawn C++ node");

    // Test with various byte patterns
    let test_cases: Vec<&[u8]> = vec![
        b"binary\x00data",         // embedded null
        "unicode_тест".as_bytes(), // unicode
        b"\x01\x02\x03\x04",       // low bytes
    ];

    for name in test_cases {
        let cpp_id = cpp_node.compute_overlay_id(name).expect("C++ failed for binary test");

        let rust_id = compat_test::overlay_id::compute_overlay_id(name);
        let rust_id_hex = hex::encode(rust_id);

        assert_eq!(
            cpp_id.to_lowercase(),
            rust_id_hex.to_lowercase(),
            "Overlay ID mismatch for binary data"
        );
    }

    cpp_node.shutdown().expect("Failed to shutdown");
}

/// Test that C++ node responds to ping
#[test]
fn test_cpp_node_ping() {
    skip_if_no_cpp!();

    let mut cpp_node = CppTestNode::spawn(14012).expect("Failed to spawn C++ node");

    cpp_node.ping().expect("Ping failed");
    println!("C++ node ping successful");

    cpp_node.shutdown().expect("Failed to shutdown");
}

/// Test getting ADNL ID from C++ node
#[test]
fn test_cpp_node_adnl_id() {
    skip_if_no_cpp!();

    let mut cpp_node = CppTestNode::spawn(14013).expect("Failed to spawn C++ node");

    let adnl_id = cpp_node.adnl_id();
    assert!(!adnl_id.is_empty(), "ADNL ID should not be empty");
    assert_eq!(adnl_id.len(), 64, "ADNL ID should be 64 hex chars (32 bytes)");

    // Verify it's valid hex
    hex::decode(adnl_id).expect("ADNL ID should be valid hex");

    println!("C++ node ADNL ID: {}", adnl_id);

    cpp_node.shutdown().expect("Failed to shutdown");
}
