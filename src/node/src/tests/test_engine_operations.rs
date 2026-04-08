/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;

#[test]
fn test_destroyed_session_ids_roundtrip() {
    let id_a = UInt256::from_slice(&[0x11; 32]);
    let id_b = UInt256::from_slice(&[0x22; 32]);
    let ids = HashSet::from([id_b.clone(), id_a.clone()]);

    let serialized = serialize_destroyed_session_ids(&ids);
    let restored = deserialize_destroyed_session_ids(&serialized).unwrap();

    assert_eq!(restored, vec![id_a, id_b], "session IDs must round-trip in sorted order");
}

#[test]
fn test_destroyed_session_ids_reject_invalid_length() {
    let data = vec![1, 0, 0, 0, 0xaa];
    let err = deserialize_destroyed_session_ids(&data).unwrap_err();
    assert!(err.to_string().contains("invalid length"), "unexpected error: {err}");
}
