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
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    types::algorithm::Algorithm,
};
use std::marker::PhantomData;

pub trait Crypto: Send + Sync {
    fn pub_key_from_pvt(&self, algo: Algorithm, pvt_key: &[u8]) -> anyhow::Result<Vec<u8>>;
    fn pub_key_from_exp(&self, algo: Algorithm, exp_key: &[u8]) -> anyhow::Result<Vec<u8>>;
    fn exp_key_from_pvt(&self, algo: Algorithm, pvt_key: &[u8]) -> anyhow::Result<ProtectedMemory>;

    fn generate_rnd(&self, algo: Algorithm, size: Option<usize>)
        -> anyhow::Result<ProtectedMemory>;

    fn sign(&self, key: &KeyMaterial, data: &[u8], algo: Algorithm) -> anyhow::Result<Vec<u8>>;

    fn verify(
        &self,
        pub_key: &[u8],
        data: &[u8],
        signature: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<()>;

    fn encrypt(
        &self,
        key: &KeyMaterial,
        plaintext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<Vec<u8>>;

    fn decrypt(
        &self,
        key: &KeyMaterial,
        ciphertext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<ProtectedMemory>;
}

pub const ED25519_PRIVATE_KEY_LENGTH: usize = 32;
pub const ED25519_PUBLIC_KEY_LENGTH: usize = 32;
pub const ED25519_EXPANDED_KEY_LENGTH: usize = 64;
pub const AES_GCM_KEY_LENGTH: usize = 32;

pub trait Ed25519Backend: Send + Sync {
    fn sign_ed25519(key: &KeyMaterial, data: &[u8]) -> anyhow::Result<Vec<u8>>;

    fn verify_ed25519(
        pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH],
        data: &[u8],
        signature: &[u8],
    ) -> anyhow::Result<()>;

    fn pub_key_from_pvt(pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH]) -> anyhow::Result<Vec<u8>>;

    fn pub_key_from_exp(exp_key: &[u8; ED25519_EXPANDED_KEY_LENGTH]) -> anyhow::Result<Vec<u8>>;

    fn exp_key_from_pvt(
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> anyhow::Result<ProtectedMemory>;
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

impl<B: Ed25519Backend + 'static> Crypto for CryptoImpl<B> {
    fn pub_key_from_pvt(&self, algo: Algorithm, pvt_key: &[u8]) -> anyhow::Result<Vec<u8>> {
        match algo {
            Algorithm::None | Algorithm::Aes256Gcm => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Ed25519 => B::pub_key_from_pvt(pvt_key.try_into()?),
        }
    }

    fn pub_key_from_exp(&self, algo: Algorithm, exp_key: &[u8]) -> anyhow::Result<Vec<u8>> {
        match algo {
            Algorithm::None | Algorithm::Aes256Gcm => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Ed25519 => B::pub_key_from_exp(exp_key.try_into()?),
        }
    }

    fn exp_key_from_pvt(&self, algo: Algorithm, pvt_key: &[u8]) -> anyhow::Result<ProtectedMemory> {
        match algo {
            Algorithm::None | Algorithm::Aes256Gcm => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Ed25519 => B::exp_key_from_pvt(pvt_key.try_into()?),
        }
    }

    fn generate_rnd(
        &self,
        algorithm: Algorithm,
        size: Option<usize>,
    ) -> anyhow::Result<ProtectedMemory> {
        let key = match algorithm {
            Algorithm::None => {
                let size = size.ok_or_else(|| {
                    anyhow::anyhow!(VaultError::invalid_key_size("Size is not set"))
                })?;

                ProtectedMemoryInner::generate_random(self.prng.as_ref(), size)?.into()
            }
            Algorithm::Aes256Gcm => {
                let size = size.unwrap_or(AES_GCM_KEY_LENGTH);
                if size != AES_GCM_KEY_LENGTH {
                    anyhow::bail!(VaultError::invalid_key_size(format!(
                        "Invalid key size for Aes256Gcm, expected {}, got {}",
                        AES_GCM_KEY_LENGTH, size
                    )));
                }

                ProtectedMemoryInner::generate_random(self.prng.as_ref(), AES_GCM_KEY_LENGTH)?
                    .into()
            }
            Algorithm::Ed25519 => {
                let size = size.unwrap_or(ED25519_PRIVATE_KEY_LENGTH);
                if size != ED25519_PRIVATE_KEY_LENGTH {
                    anyhow::bail!(VaultError::invalid_key_size(format!(
                        "Invalid key size for Ed25519, expected {}, got {}",
                        ED25519_PRIVATE_KEY_LENGTH, size
                    )));
                }

                ProtectedMemoryInner::generate_random(
                    self.prng.as_ref(),
                    ED25519_PRIVATE_KEY_LENGTH,
                )?
                .into()
            }
        };

        Ok(key)
    }

    fn sign(&self, key: &KeyMaterial, data: &[u8], algo: Algorithm) -> anyhow::Result<Vec<u8>> {
        match algo {
            Algorithm::Aes256Gcm | Algorithm::None => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Ed25519 => B::sign_ed25519(key, data),
        }
    }

    fn verify(
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
            Algorithm::Ed25519 => B::verify_ed25519(pub_key.try_into()?, data, signature),
        }
    }

    fn encrypt(
        &self,
        key: &KeyMaterial,
        plaintext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<Vec<u8>> {
        match algo {
            Algorithm::Ed25519 | Algorithm::None => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Aes256Gcm => aes_gcm_encrypt(key, plaintext, self.prng.as_ref()),
        }
    }

    fn decrypt(
        &self,
        key: &KeyMaterial,
        ciphertext: &[u8],
        algo: Algorithm,
    ) -> anyhow::Result<ProtectedMemory> {
        match algo {
            Algorithm::Ed25519 | Algorithm::None => {
                anyhow::bail!(VaultError::unsupported_algorithm(algo))
            }
            Algorithm::Aes256Gcm => aes_gcm_decrypt(key, ciphertext),
        }
    }
}
