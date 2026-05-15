/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::{
        crypto_trait::{
            Ed25519Backend, ED25519_EXPANDED_KEY_LENGTH, ED25519_PRIVATE_KEY_LENGTH,
            ED25519_PUBLIC_KEY_LENGTH,
        },
        default_impl::DefaultEd25519,
        key_material::KeyMaterial,
    },
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
};

fn make_test_private_key() -> [u8; ED25519_PRIVATE_KEY_LENGTH] {
    let mut key = [0u8; ED25519_PRIVATE_KEY_LENGTH];
    for (i, b) in key.iter_mut().enumerate() {
        *b = i as u8;
    }
    key
}

/// Core bug test: pub_key_from_pvt(k) must equal pub_key_from_exp(exp_key_from_pvt(k))
#[test]
fn test_exp_key_from_pvt_roundtrip_public_key() {
    let pvt_key = make_test_private_key();

    // Direct: private key → public key
    let pub_key_direct = DefaultEd25519::pub_key_from_pvt(&pvt_key).unwrap();

    // Round-trip: private key → expanded key → public key
    let exp_key_mem = DefaultEd25519::exp_key_from_pvt(&pvt_key).unwrap();
    let exp_key_lock = exp_key_mem.lock().unwrap();
    assert_eq!(exp_key_lock.len(), ED25519_EXPANDED_KEY_LENGTH);

    let exp_key_bytes: [u8; ED25519_EXPANDED_KEY_LENGTH] =
        exp_key_lock.as_ref().try_into().unwrap();
    let pub_key_roundtrip = DefaultEd25519::pub_key_from_exp(&exp_key_bytes).unwrap();

    assert_eq!(
        pub_key_direct, pub_key_roundtrip,
        "pub_key_from_pvt(k) != pub_key_from_exp(exp_key_from_pvt(k)): \
         scalar representation mismatch between from_bits_clamped and from_bytes_mod_order"
    );
}

/// Verify that signatures from private key and its expanded form match
#[test]
fn test_exp_key_from_pvt_roundtrip_sign() {
    let pvt_key = make_test_private_key();
    let data = b"test message for signing";

    // Create KeyMaterial from private key (non-expanded)
    let pvt_mem: ProtectedMemory = ProtectedMemoryInner::from_slice(&pvt_key).unwrap().into();
    let km_pvt = KeyMaterial::new(Some(pvt_mem), None).unwrap();

    let sig_pvt = DefaultEd25519::sign_ed25519(&km_pvt, data).unwrap();

    // Create KeyMaterial from expanded key
    let exp_key_mem = DefaultEd25519::exp_key_from_pvt(&pvt_key).unwrap();
    let exp_key_lock = exp_key_mem.lock().unwrap();
    let exp_key_bytes: [u8; ED25519_EXPANDED_KEY_LENGTH] =
        exp_key_lock.as_ref().try_into().unwrap();
    drop(exp_key_lock);

    let exp_mem: ProtectedMemory = ProtectedMemoryInner::from_slice(&exp_key_bytes).unwrap().into();
    let pub_key_direct = DefaultEd25519::pub_key_from_pvt(&pvt_key).unwrap();
    let km_exp = KeyMaterial::new_exp_pub(exp_mem, pub_key_direct.clone()).unwrap();

    let sig_exp = DefaultEd25519::sign_ed25519(&km_exp, data).unwrap();

    assert_eq!(sig_pvt, sig_exp, "Signatures from private key and expanded key must match");

    // Verify both signatures
    let pub_key: [u8; ED25519_PUBLIC_KEY_LENGTH] = pub_key_direct.try_into().unwrap();
    DefaultEd25519::verify_ed25519(&pub_key, data, &sig_pvt).unwrap();
    DefaultEd25519::verify_ed25519(&pub_key, data, &sig_exp).unwrap();
}

/// Verify expanded key length is correct
#[test]
fn test_exp_key_from_pvt_length() {
    let pvt_key = make_test_private_key();
    let exp_key_mem = DefaultEd25519::exp_key_from_pvt(&pvt_key).unwrap();
    let exp_key_lock = exp_key_mem.lock().unwrap();
    assert_eq!(exp_key_lock.len(), ED25519_EXPANDED_KEY_LENGTH);
}
