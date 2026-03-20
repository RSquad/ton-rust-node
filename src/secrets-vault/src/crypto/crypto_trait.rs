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
        aes_gcm::{decrypt as aes_gcm_decrypt, encrypt as aes_gcm_encrypt},
        key_material::KeyMaterial,
        prng::Prng,
    },
    errors::error::VaultError,
    memory::protected_memory::ProtectedMemory,
    types::algorithm::Algorithm,
};
use std::marker::PhantomData;

#[async_trait::async_trait]
pub trait Crypto: Send + Sync {
    async fn pub_key_from_pvt(&self, algo: Algorithm, pvt_key: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn generate_rnd(
        &self,
        algo: Algorithm,
        size: Option<usize>,
    ) -> anyhow::Result<ProtectedMemory>;

    async fn sign(
        &self,
        key: &KeyMaterial,
        data: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<Vec<u8>>;

    async fn verify(
        &self,
        pub_key: &[u8],
        data: &[u8],
        signature: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<()>;

    async fn encrypt(
        &self,
        key: &KeyMaterial,
        plaintext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<Vec<u8>>;

    async fn decrypt(
        &self,
        key: &KeyMaterial,
        ciphertext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<ProtectedMemory>;
}

pub const ED25519_PRIVATE_KEY_LENGTH: usize = 32;
pub const ED25519_PUBLIC_KEY_LENGTH: usize = 32;
pub const AES_GCM_KEY_LENGTH: usize = 32;

#[async_trait::async_trait]
pub trait Ed25519Backend: Send + Sync {
    async fn sign_ed25519(key: &KeyMaterial, data: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn verify_ed25519(
        pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH],
        data: &[u8],
        signature: &[u8],
    ) -> anyhow::Result<()>;
    async fn pub_key_from_seed(
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> anyhow::Result<Vec<u8>>;
}

pub struct CryptoImpl<B: Ed25519Backend> {
    prng: Box<dyn Prng>,
    _marker: PhantomData<B>,
}

impl<B: Ed25519Backend> CryptoImpl<B> {
    pub fn new(prng: Box<dyn Prng>) -> Self {
        Self { prng, _marker: PhantomData }
    }
}

#[async_trait::async_trait]
impl<B: Ed25519Backend + 'static> Crypto for CryptoImpl<B> {
    async fn pub_key_from_pvt(&self, algo: Algorithm, pvt_key: &[u8]) -> anyhow::Result<Vec<u8>> {
        match algo {
            Algorithm::None | Algorithm::Aes256Gcm => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Ed25519 => B::pub_key_from_seed(pvt_key.try_into()?).await,
        }
    }

    async fn generate_rnd(
        &self,
        algorithm: Algorithm,
        size: Option<usize>,
    ) -> anyhow::Result<ProtectedMemory> {
        let key = match algorithm {
            Algorithm::None => {
                let size = size.ok_or_else(|| {
                    anyhow::anyhow!(VaultError::invalid_key_size("Size is not set"))
                })?;

                ProtectedMemory::generate_random(self.prng.as_ref(), size).await?
            }
            Algorithm::Aes256Gcm => {
                let size = size.unwrap_or(AES_GCM_KEY_LENGTH);
                if size != AES_GCM_KEY_LENGTH {
                    anyhow::bail!(VaultError::invalid_key_size(format!(
                        "Invalid key size for Aes256Gcm, expected {}, got {}",
                        AES_GCM_KEY_LENGTH, size
                    )));
                }

                ProtectedMemory::generate_random(self.prng.as_ref(), AES_GCM_KEY_LENGTH).await?
            }
            Algorithm::Ed25519 => {
                let size = size.unwrap_or(ED25519_PRIVATE_KEY_LENGTH);
                if size != ED25519_PRIVATE_KEY_LENGTH {
                    anyhow::bail!(VaultError::invalid_key_size(format!(
                        "Invalid key size for Ed25519, expected {}, got {}",
                        ED25519_PRIVATE_KEY_LENGTH, size
                    )));
                }

                ProtectedMemory::generate_random(self.prng.as_ref(), ED25519_PRIVATE_KEY_LENGTH)
                    .await?
            }
        };

        Ok(key)
    }

    async fn sign(
        &self,
        key: &KeyMaterial,
        data: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<Vec<u8>> {
        match algo {
            Algorithm::Aes256Gcm | Algorithm::None => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Ed25519 => B::sign_ed25519(key, data).await,
        }
    }

    async fn verify(
        &self,
        pub_key: &[u8],
        data: &[u8],
        signature: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<()> {
        match algo {
            Algorithm::Aes256Gcm | Algorithm::None => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Ed25519 => B::verify_ed25519(pub_key.try_into()?, data, signature).await,
        }
    }

    async fn encrypt(
        &self,
        key: &KeyMaterial,
        plaintext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<Vec<u8>> {
        match algo {
            Algorithm::Ed25519 | Algorithm::None => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Aes256Gcm => aes_gcm_encrypt(key, plaintext, self.prng.as_ref()).await,
        }
    }

    async fn decrypt(
        &self,
        key: &KeyMaterial,
        ciphertext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<ProtectedMemory> {
        match algo {
            Algorithm::Ed25519 | Algorithm::None => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Aes256Gcm => aes_gcm_decrypt(key, ciphertext).await,
        }
    }
}
