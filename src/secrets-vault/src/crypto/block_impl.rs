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
use ton_block::{
    ed25519_create_private_key, ed25519_create_public_key, ed25519_expand_private_key,
    ed25519_sign, ed25519_verify,
};

pub struct BlockEd25519;

#[async_trait::async_trait]
impl Ed25519Backend for BlockEd25519 {
    async fn sign_ed25519(key: &KeyMaterial, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let secret_key = key
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

        let key_lock = secret_key.lock().await?;
        let key_data: &[u8] = &key_lock;

        if key_data.len() != ED25519_PRIVATE_KEY_LENGTH {
            anyhow::bail!(VaultError::invalid_key_size(format!(
                "Invalid key size for Ed25519, expected {}, got {}",
                ED25519_PRIVATE_KEY_LENGTH,
                key_data.len()
            )));
        }

        let pvt_key_exp = ed25519_expand_private_key(&ed25519_create_private_key(&key_lock)?)?;
        ed25519_sign(pvt_key_exp.to_bytes().as_ref(), None, data)
    }

    async fn verify_ed25519(
        pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH],
        data: &[u8],
        signature: &[u8],
    ) -> anyhow::Result<()> {
        ed25519_verify(pub_key, data, signature)
            .map_err(|_| VaultError::invalid_signature("Signature verification failed"))?;
        Ok(())
    }

    async fn pub_key_from_seed(
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> anyhow::Result<Vec<u8>> {
        let pvt_key = ed25519_create_private_key(pvt_key)?;
        let pvt_key_exp = ed25519_expand_private_key(&pvt_key)?;
        let pub_key = ed25519_create_public_key(&pvt_key_exp)?;
        Ok(pub_key.as_bytes().to_vec())
    }
}
