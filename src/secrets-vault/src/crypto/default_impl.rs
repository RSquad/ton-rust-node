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
        crypto_trait::{Ed25519Backend, ED25519_PRIVATE_KEY_LENGTH, ED25519_PUBLIC_KEY_LENGTH},
        key_material::KeyMaterial,
    },
    errors::error::VaultError,
};
use ed25519_dalek::{Signer, Verifier};

pub struct DefaultEd25519;

#[async_trait::async_trait]
impl Ed25519Backend for DefaultEd25519 {
    async fn sign_ed25519(key: &KeyMaterial, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let pvt_key = key
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Failed to sign"))?;
        let pvt_key_lock = pvt_key.lock().await?;
        let pvt_key_data: &[u8] = &pvt_key_lock;

        if pvt_key_data.len() != ED25519_PRIVATE_KEY_LENGTH {
            anyhow::bail!(VaultError::invalid_key_size(format!(
                "Invalid key size for Ed25519, expected {}, got {}",
                ED25519_PRIVATE_KEY_LENGTH,
                pvt_key_data.len()
            )));
        }

        let sign_key = ed25519_dalek::SigningKey::from_bytes(pvt_key_data.try_into()?);
        let signature = sign_key.sign(data);
        Ok(signature.to_bytes().to_vec())
    }

    async fn verify_ed25519(
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

    async fn pub_key_from_seed(pvt_key: &[u8; 32]) -> anyhow::Result<Vec<u8>> {
        let pvt_key = ed25519_dalek::SigningKey::from_bytes(pvt_key);
        Ok(pvt_key.verifying_key().as_bytes().to_vec())
    }
}
