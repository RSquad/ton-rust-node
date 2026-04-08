/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! Overlay ID calculation utilities
//!
//! This module provides functions to compute overlay IDs in a way compatible
//! with the cpp implementation.

use ton_api::{serialize_boxed, ton::pub_::publickey::Overlay as OverlayKey, IntoBoxed};
use ton_block::sha256_digest;

/// Compute overlay short ID from overlay name (same as C++ OverlayIdFull::compute_short_id)
///
/// The overlay ID is computed by:
/// 1. Creating an "overlay pubkey" from the name using the overlay key type
/// 2. Computing the short ID (SHA256 hash) of that boxed pubkey
///
/// The input `name` is the raw TL bytes that would be passed to OverlayIdFull.
pub fn compute_overlay_id(name: &[u8]) -> [u8; 32] {
    // Use the same approach as adnl/src/overlay/mod.rs:
    // OverlayKey { name: ... } then hash_boxed
    let overlay_key = OverlayKey { name: name.to_vec().into() };
    let boxed = overlay_key.into_boxed();
    let serialized = serialize_boxed(&boxed).expect("serialize overlay key");
    sha256_digest(&serialized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlay_id_basic() {
        let name = b"test_overlay";
        let id = compute_overlay_id(name);

        // The ID should be a valid 32-byte hash
        assert_eq!(id.len(), 32);

        // Same input should produce same output
        let id2 = compute_overlay_id(name);
        assert_eq!(id, id2);

        // Different input should produce different output
        let id3 = compute_overlay_id(b"other_overlay");
        assert_ne!(id, id3);
    }

    #[test]
    fn test_overlay_id_empty() {
        let id = compute_overlay_id(b"");
        assert_eq!(id.len(), 32);
    }

    #[test]
    fn test_overlay_id_long_name() {
        // Test with name > 254 bytes
        let name = vec![b'x'; 300];
        let id = compute_overlay_id(&name);
        assert_eq!(id.len(), 32);
    }
}
