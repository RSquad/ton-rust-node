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
            Crypto, CryptoImpl, Ed25519Backend, ED25519_EXPANDED_KEY_LENGTH,
            ED25519_PRIVATE_KEY_LENGTH, ED25519_PUBLIC_KEY_LENGTH,
        },
        factory::CryptoFactory,
        key_material::KeyMaterial,
        prng_chacha20::PrngChacha20,
    },
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
};
use std::sync::Arc;
use ton_block::{
    ed25519_create_expanded_private_key, ed25519_create_private_key, ed25519_create_public_key,
    ed25519_expand_private_key, ed25519_sign, ed25519_verify,
};

pub struct BlockEd25519;

impl Ed25519Backend for BlockEd25519 {
    fn sign_ed25519(key: &KeyMaterial, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let secret_key = key
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

        let key_lock = secret_key.lock()?;
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

    fn verify_ed25519(
        pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH],
        data: &[u8],
        signature: &[u8],
    ) -> anyhow::Result<()> {
        ed25519_verify(pub_key, data, signature)
            .map_err(|_| VaultError::invalid_signature("Signature verification failed"))?;
        Ok(())
    }

    fn pub_key_from_pvt(pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH]) -> anyhow::Result<Vec<u8>> {
        let pvt_key = ed25519_create_private_key(pvt_key)?;
        let pvt_key_exp = ed25519_expand_private_key(&pvt_key)?;
        let pub_key = ed25519_create_public_key(&pvt_key_exp)?;
        Ok(pub_key.as_bytes().to_vec())
    }

    fn pub_key_from_exp(exp_key: &[u8; ED25519_EXPANDED_KEY_LENGTH]) -> anyhow::Result<Vec<u8>> {
        let pub_key = ed25519_create_public_key(&ed25519_create_expanded_private_key(exp_key)?)?;
        Ok(pub_key.as_bytes().to_vec())
    }

    fn exp_key_from_pvt(
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> anyhow::Result<ProtectedMemory> {
        let exp_key = ed25519_expand_private_key(&ed25519_create_private_key(pvt_key)?)?;
        Ok(ProtectedMemoryInner::from_slice(exp_key.as_bytes())?.into())
    }
}

#[derive(Debug)]
pub struct BlockCryptoFactory {}

impl CryptoFactory for BlockCryptoFactory {
    fn new_crypto(&self) -> anyhow::Result<Arc<dyn Crypto>> {
        let prng = Box::new(PrngChacha20 {});
        Ok(Arc::new(CryptoImpl::<BlockEd25519>::new(prng)))
    }
}
