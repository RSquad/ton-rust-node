/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use super::*;
use ton_block::Ed25519KeyOption;

fn make_test_key() -> Arc<dyn KeyOption> {
    Ed25519KeyOption::generate().unwrap()
}

fn make_validator_node(
    public_key: Arc<dyn KeyOption>,
    adnl_key: Arc<dyn KeyOption>,
) -> CatchainNode {
    CatchainNode { public_key, adnl_id: adnl_key.id().clone() }
}

#[test]
fn test_select_local_validator_candidate_matches_pubkey_and_adnl() {
    let validator_key = make_test_key();
    let adnl_key = make_test_key();
    let validator = make_validator_node(validator_key.clone(), adnl_key.clone());
    let validator_key_ids = vec![validator_key.id().clone()];
    let validator_adnl_key_ids = vec![adnl_key.id().clone()];

    let (local_validator, adnl_missing) = select_local_validator_candidate(
        std::slice::from_ref(&validator),
        &validator_key_ids,
        &validator_adnl_key_ids,
    );

    let local_validator = local_validator.expect("local validator should be selected");
    assert_eq!(local_validator.public_key.id(), validator_key.id());
    assert_eq!(local_validator.adnl_id, adnl_key.id().clone());
    assert!(!adnl_missing);
}

#[test]
fn test_select_local_validator_candidate_matches_pubkey_when_adnl_missing() {
    let validator_key = make_test_key();
    let chain_adnl_key = make_test_key();
    let local_adnl_key = make_test_key();
    let validator = make_validator_node(validator_key.clone(), chain_adnl_key.clone());
    let validator_key_ids = vec![validator_key.id().clone()];
    let validator_adnl_key_ids = vec![local_adnl_key.id().clone()];

    let (local_validator, adnl_missing) = select_local_validator_candidate(
        std::slice::from_ref(&validator),
        &validator_key_ids,
        &validator_adnl_key_ids,
    );

    let local_validator = local_validator.expect("pubkey membership should select the validator");
    assert_eq!(local_validator.public_key.id(), validator_key.id());
    assert_eq!(local_validator.adnl_id, chain_adnl_key.id().clone());
    assert!(adnl_missing);
}

#[test]
fn test_select_local_validator_candidate_accepts_pubkey_adnl_fallback() {
    let validator_key = make_test_key();
    let chain_adnl_key = make_test_key();
    let validator = make_validator_node(validator_key.clone(), chain_adnl_key);
    let validator_key_ids = vec![validator_key.id().clone()];
    // C++ parity fallback: allow validator pubkey short-id to serve as ADNL identity.
    let validator_adnl_key_ids = vec![validator_key.id().clone()];

    let (local_validator, adnl_missing) = select_local_validator_candidate(
        std::slice::from_ref(&validator),
        &validator_key_ids,
        &validator_adnl_key_ids,
    );

    let local_validator = local_validator.expect("pubkey membership should select validator");
    assert_eq!(local_validator.public_key.id(), validator_key.id());
    assert!(!adnl_missing);
}

#[test]
fn test_select_local_validator_candidate_returns_none_without_pubkey_match() {
    let validator_key = make_test_key();
    let adnl_key = make_test_key();
    let other_validator_key = make_test_key();
    let validator = make_validator_node(validator_key, adnl_key.clone());
    let validator_key_ids = vec![other_validator_key.id().clone()];
    let validator_adnl_key_ids = vec![adnl_key.id().clone()];

    let (local_validator, adnl_missing) = select_local_validator_candidate(
        std::slice::from_ref(&validator),
        &validator_key_ids,
        &validator_adnl_key_ids,
    );

    assert!(local_validator.is_none());
    assert!(!adnl_missing);
}

/// Verifies that selection follows local-key order, not validator-list order.
///
/// This mirrors C++ `get_validator()` which iterates `temp_keys_` and returns the first
/// local key that is present in the validator set, without ADNL consideration.
#[test]
fn test_select_local_validator_candidate_uses_first_local_key_match() {
    let key_a = make_test_key();
    let key_b = make_test_key();
    let adnl_a = make_test_key();
    let adnl_b = make_test_key();
    let local_adnl = make_test_key();

    let val_a = make_validator_node(key_a.clone(), adnl_a.clone());
    let val_b = make_validator_node(key_b.clone(), adnl_b.clone());
    let validators = vec![val_a, val_b];

    // Case 1: validator list order is [A, B], but local key order is [B, A].
    // Rust must follow local key order to match C++ temp_keys_ iteration.
    let local_key_ids = vec![key_b.id().clone(), key_a.id().clone()];
    let local_adnl_ids = vec![adnl_b.id().clone(), local_adnl.id().clone()];

    let (selected, adnl_missing) =
        select_local_validator_candidate(&validators, &local_key_ids, &local_adnl_ids);

    let selected = selected.expect("should select first local-key match");
    assert_eq!(selected.public_key.id(), key_b.id(), "must follow local key order");
    assert!(!adnl_missing, "selected validator has a ready ADNL key");

    // Case 2: ADNL readiness still must not change key selection.
    let unrelated_adnl = make_test_key();
    let local_adnl_ids_none = vec![unrelated_adnl.id().clone(), adnl_a.id().clone()];

    let (selected2, adnl_missing2) =
        select_local_validator_candidate(&validators, &local_key_ids, &local_adnl_ids_none);

    let selected2 = selected2.expect("should still select first local-key match");
    assert_eq!(selected2.public_key.id(), key_b.id(), "ADNL readiness must not affect key order");
    assert!(adnl_missing2);
}
