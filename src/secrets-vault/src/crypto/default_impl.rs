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
        key_material::KeyMaterial,
    },
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
};
use ed25519_dalek::{Signer, Verifier};
use sha2::Digest;
use zeroize::Zeroize;

pub struct DefaultEd25519;

impl Ed25519Backend for DefaultEd25519 {
    fn sign_ed25519(key: &KeyMaterial, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let secret_key = key
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Failed to sign"))?;
        let key_lock = secret_key.lock()?;
        let is_expanded =
            key.tags.as_ref().is_some_and(|t| t.iter().any(|t| t == KeyMaterial::TAG_EXPANDED_KEY));

        if is_expanded {
            if key_lock.len() != ED25519_EXPANDED_KEY_LENGTH {
                anyhow::bail!(VaultError::invalid_key_size(format!(
                    "Invalid expanded key size for Ed25519, expected {}, got {}",
                    ED25519_EXPANDED_KEY_LENGTH,
                    key_lock.len()
                )));
            }
            let exp_key =
                ed25519_dalek::hazmat::ExpandedSecretKey::from_bytes(key_lock.as_ref().try_into()?);
            let pub_key = ed25519_dalek::VerifyingKey::from(&exp_key);
            let signature =
                ed25519_dalek::hazmat::raw_sign::<sha2::Sha512>(&exp_key, data, &pub_key);
            Ok(signature.to_bytes().to_vec())
        } else {
            if key_lock.len() != ED25519_PRIVATE_KEY_LENGTH {
                anyhow::bail!(VaultError::invalid_key_size(format!(
                    "Invalid key size for Ed25519, expected {}, got {}",
                    ED25519_PRIVATE_KEY_LENGTH,
                    key_lock.len()
                )));
            }
            let sign_key = ed25519_dalek::SigningKey::from_bytes(key_lock.as_ref().try_into()?);
            let signature = sign_key.sign(data);
            Ok(signature.to_bytes().to_vec())
        }
    }

    fn verify_ed25519(
        pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH],
        data: &[u8],
        signature: &[u8],
    ) -> anyhow::Result<()> {
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(pub_key)
            .map_err(|_| VaultError::invalid_public_key("Invalid Ed25519 public key"))?;

        let sig_bytes: [u8; 64] = signature
            .try_into()
            .map_err(|_| VaultError::invalid_signature("Invalid signature length"))?;

        let signature = ed25519_dalek::Signature::from_bytes(&sig_bytes);

        verifying_key
            .verify(data, &signature)
            .map_err(|_| VaultError::invalid_signature("Signature verification failed"))?;

        Ok(())
    }

    fn pub_key_from_pvt(pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH]) -> anyhow::Result<Vec<u8>> {
        let pvt_key = ed25519_dalek::SigningKey::from_bytes(pvt_key);
        Ok(pvt_key.verifying_key().as_bytes().to_vec())
    }

    fn pub_key_from_exp(exp_key: &[u8; ED25519_EXPANDED_KEY_LENGTH]) -> anyhow::Result<Vec<u8>> {
        let expanded = ed25519_dalek::hazmat::ExpandedSecretKey::from_bytes(exp_key);
        let verifying_key = ed25519_dalek::VerifyingKey::from(&expanded);
        Ok(verifying_key.to_bytes().to_vec())
    }

    fn exp_key_from_pvt(
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> anyhow::Result<ProtectedMemory> {
        let mut hash = sha2::Sha512::digest(pvt_key);
        let exp_key: ProtectedMemory = ProtectedMemoryInner::from_slice(hash.as_ref())?.into();
        hash.zeroize();

        Ok(exp_key)
    }
}
