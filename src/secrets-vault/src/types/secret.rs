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
        blob_in_memory::BlobInMemory, crypto_trait::Crypto, key_material::KeyMaterial,
        key_pair_in_memory::KeyPairInMemory, symmetric_key_in_memory::SymmetricKeyInMemory,
    },
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    types::{algorithm::Algorithm, metadata::Metadata, payload::PayloadType, secret_id::SecretId},
};
use core::fmt;
use std::sync::Arc;

pub trait Blob: Send + Sync + downcast_rs::Downcast {
    fn id(&self) -> Option<&SecretId>;
    fn metadata(&self) -> &Metadata;
    fn metadata_mut(&mut self) -> &mut Metadata;
    fn data(&self) -> &ProtectedMemory;
    fn eq_blob(&self, other: &dyn Blob) -> anyhow::Result<bool> {
        Ok((self.metadata() == other.metadata()) && self.data().eq_pm(other.data())?)
    }
}

downcast_rs::impl_downcast!(Blob);

#[async_trait::async_trait]
pub trait KeyPair: Send + Sync + downcast_rs::Downcast {
    fn id(&self) -> Option<&SecretId>;
    fn metadata(&self) -> &Metadata;
    fn metadata_mut(&mut self) -> &mut Metadata;
    fn extractable(&self) -> bool;
    fn public_key(&self) -> Option<&[u8]>;
    fn private_key(&self) -> anyhow::Result<&ProtectedMemory>;
    fn expanded_key(&self) -> anyhow::Result<ProtectedMemory>;
    async fn sign(&self, data: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn verify(&self, data: &[u8], signature: &[u8]) -> anyhow::Result<()>;
    fn serialize(&self) -> anyhow::Result<ProtectedMemory>;
    fn eq_keypair(&self, other: &dyn KeyPair) -> anyhow::Result<bool> {
        let extractable = self.extractable();

        let mut is_eq = (extractable == other.extractable())
            && (self.metadata() == other.metadata())
            && (self.public_key() == other.public_key());

        if is_eq && extractable {
            is_eq &= self.private_key()?.eq_pm(other.private_key()?)?;
        }

        Ok(is_eq)
    }
}

downcast_rs::impl_downcast!(KeyPair);

#[async_trait::async_trait]
pub trait SymmetricKey: Send + Sync + downcast_rs::Downcast {
    fn id(&self) -> Option<&SecretId>;
    fn metadata(&self) -> &Metadata;
    fn metadata_mut(&mut self) -> &mut Metadata;
    fn extractable(&self) -> anyhow::Result<bool>;
    fn key(&self) -> anyhow::Result<&ProtectedMemory>;
    async fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn decrypt(&self, ciphertext: &[u8]) -> anyhow::Result<Vec<u8>>;
    fn serialize(&self) -> anyhow::Result<ProtectedMemory>;
    fn eq_key(&self, other: &dyn SymmetricKey) -> anyhow::Result<bool> {
        let extractable = self.extractable()?;

        let mut is_eq =
            (self.metadata() == other.metadata()) && (extractable == other.extractable()?);

        if extractable {
            is_eq &= self.key()?.eq_pm(other.key()?)?;
        }

        Ok(is_eq)
    }
}

downcast_rs::impl_downcast!(SymmetricKey);

pub enum Secret {
    Blob { blob: Box<dyn Blob> },
    KeyPair { keypair: Box<dyn KeyPair> },
    SymmetricKey { key: Box<dyn SymmetricKey> },
}

impl Secret {
    pub fn id(&self) -> Option<&SecretId> {
        match self {
            Secret::Blob { blob } => blob.id(),
            Secret::KeyPair { keypair } => keypair.id(),
            Secret::SymmetricKey { key } => key.id(),
        }
    }

    pub fn metadata(&self) -> &Metadata {
        match self {
            Secret::Blob { blob } => blob.metadata(),
            Secret::KeyPair { keypair } => keypair.metadata(),
            Secret::SymmetricKey { key } => key.metadata(),
        }
    }

    pub fn metadata_mut(&mut self) -> &mut Metadata {
        match self {
            Secret::Blob { blob } => blob.metadata_mut(),
            Secret::KeyPair { keypair } => keypair.metadata_mut(),
            Secret::SymmetricKey { key } => key.metadata_mut(),
        }
    }

    pub fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        match self {
            Secret::Blob { blob } => blob.data().try_clone(),
            Secret::KeyPair { keypair } => keypair.serialize(),
            Secret::SymmetricKey { key } => key.serialize(),
        }
    }

    pub fn variant_name(&self) -> &'static str {
        match self {
            Secret::Blob { .. } => "Blob",
            Secret::KeyPair { .. } => "KeyPair",
            Secret::SymmetricKey { .. } => "SymmetricKey",
        }
    }

    pub fn as_blob(&self) -> anyhow::Result<&dyn Blob> {
        match self {
            Secret::Blob { blob } => Ok(blob.as_ref()),
            _ => {
                anyhow::bail!(VaultError::wrong_secret_type(format!("Expected Blob got {}", self)))
            }
        }
    }

    pub fn as_keypair(&self) -> anyhow::Result<&dyn KeyPair> {
        match self {
            Secret::KeyPair { keypair } => Ok(keypair.as_ref()),
            _ => anyhow::bail!(VaultError::wrong_secret_type(format!(
                "Expected KeyPair got {}",
                self
            ))),
        }
    }

    pub fn as_symmetric_key(&self) -> anyhow::Result<&dyn SymmetricKey> {
        match self {
            Secret::SymmetricKey { key } => Ok(key.as_ref()),
            _ => anyhow::bail!(VaultError::wrong_secret_type(format!(
                "Expected SymmetricKey got {}",
                self
            ))),
        }
    }

    pub fn eq_secret(&self, other: &Self) -> anyhow::Result<bool> {
        match (self, other) {
            (Secret::Blob { blob: a }, Secret::Blob { blob: b }) => a.eq_blob(b.as_ref()),
            (Secret::KeyPair { keypair: a }, Secret::KeyPair { keypair: b }) => {
                a.eq_keypair(b.as_ref())
            }
            (Secret::SymmetricKey { key: a }, Secret::SymmetricKey { key: b }) => {
                a.eq_key(b.as_ref())
            }
            _ => Ok(false),
        }
    }
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.variant_name())
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Secret::{}", self.variant_name())
    }
}

pub enum SecretDataType {
    Raw,
    Secret,
    Ed25519PubKey,
    Ed25519PvtKey,
    Ed25519ExpKey,
}

impl SecretDataType {
    pub fn from_algo(algorithm: Algorithm) -> Self {
        match algorithm {
            Algorithm::None => Self::Raw,
            Algorithm::Aes256Gcm => Self::Secret,
            Algorithm::Ed25519 => Self::Ed25519PvtKey,
        }
    }
}

impl fmt::Display for SecretDataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            SecretDataType::Raw => "Raw",
            SecretDataType::Secret => "Secret",
            SecretDataType::Ed25519PubKey => "Ed25519 Public Key",
            SecretDataType::Ed25519PvtKey => "Ed25519 Private Key",
            SecretDataType::Ed25519ExpKey => "Ed25519 Expanded Key",
        };
        write!(f, "{}", label)
    }
}

pub struct SecretInMemoryFactory {}

impl SecretInMemoryFactory {
    pub fn deserialize(
        data: ProtectedMemory,
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        let secret = match metadata.algorithm.payload_type() {
            PayloadType::Blob => {
                Secret::Blob { blob: Box::new(BlobInMemory::new(&metadata, data)) }
            }
            PayloadType::KeyPair => {
                let key_material = KeyMaterial::deserialize(&data.lock()?)?;
                let keypair = Box::new(KeyPairInMemory::new(&metadata, key_material, crypto));
                Secret::KeyPair { keypair }
            }
            PayloadType::SymmetricKey => {
                let key_material = KeyMaterial::deserialize(&data.lock()?)?;
                let secret = Secret::SymmetricKey {
                    key: Box::new(SymmetricKeyInMemory::new(&metadata, key_material)),
                };

                return Ok(secret);
            }
        };

        Ok(secret)
    }

    pub fn from_raw_data(
        secret_data: &[u8],
        secret_data_type: SecretDataType,
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        let secret_data_pm: ProtectedMemory = ProtectedMemoryInner::from_slice(secret_data)?.into();
        Self::from_protected_data(secret_data_pm, secret_data_type, metadata, crypto)
    }

    pub fn new_raw(
        secret_data: &[u8],
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_raw_data(secret_data, SecretDataType::Raw, metadata, crypto)
    }

    pub fn new_secret(
        secret_data: &[u8],
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_raw_data(secret_data, SecretDataType::Secret, metadata, crypto)
    }

    pub fn new_ed25519_pubkey(
        pub_key: &[u8],
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_raw_data(pub_key, SecretDataType::Ed25519PubKey, metadata, crypto)
    }

    pub fn new_ed25519_pvtkey(
        pvt_key: &[u8],
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_raw_data(pvt_key, SecretDataType::Ed25519PvtKey, metadata, crypto)
    }

    pub fn new_ed25519_expkey(
        exp_key: &[u8],
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_raw_data(exp_key, SecretDataType::Ed25519ExpKey, metadata, crypto)
    }

    pub fn new_raw_protected(
        secret_data: ProtectedMemory,
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_protected_data(secret_data, SecretDataType::Raw, metadata, crypto)
    }

    pub fn new_secret_protected(
        secret_data: ProtectedMemory,
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_protected_data(secret_data, SecretDataType::Secret, metadata, crypto)
    }

    pub fn new_ed25519_pvtkey_protected(
        pvt_key: ProtectedMemory,
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_protected_data(pvt_key, SecretDataType::Ed25519PvtKey, metadata, crypto)
    }

    pub fn new_ed25519_expkey_protected(
        exp_key: ProtectedMemory,
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        Self::from_protected_data(exp_key, SecretDataType::Ed25519ExpKey, metadata, crypto)
    }

    pub fn from_protected_data(
        secret_data: ProtectedMemory,
        secret_data_type: SecretDataType,
        metadata: Metadata,
        crypto: Arc<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        let secret = match metadata.algorithm.payload_type() {
            PayloadType::Blob => {
                Secret::Blob { blob: Box::new(BlobInMemory::new(&metadata, secret_data)) }
            }
            PayloadType::KeyPair => {
                if metadata.algorithm != Algorithm::Ed25519 {
                    anyhow::bail!(VaultError::unsupported_algorithm(metadata.algorithm));
                }

                let keypair: Box<dyn KeyPair> = match secret_data_type {
                    SecretDataType::Raw | SecretDataType::Secret => {
                        anyhow::bail!(VaultError::wrong_secret_type(format!(
                            "failed to create KeyPair with secret type {}",
                            secret_data_type
                        )))
                    }
                    SecretDataType::Ed25519PubKey => Box::new(KeyPairInMemory::new(
                        &metadata,
                        KeyMaterial::new_pub_key(secret_data.lock()?.to_vec())?,
                        crypto,
                    )),
                    SecretDataType::Ed25519PvtKey => {
                        let pub_key =
                            crypto.pub_key_from_pvt(Algorithm::Ed25519, &secret_data.lock()?)?;

                        Box::new(KeyPairInMemory::new(
                            &metadata,
                            KeyMaterial::new_pvt_pub(secret_data, pub_key)?,
                            crypto,
                        ))
                    }
                    SecretDataType::Ed25519ExpKey => {
                        let pub_key =
                            crypto.pub_key_from_exp(Algorithm::Ed25519, &secret_data.lock()?)?;

                        Box::new(KeyPairInMemory::new(
                            &metadata,
                            KeyMaterial::new_exp_pub(secret_data, pub_key)?,
                            crypto,
                        ))
                    }
                };

                Secret::KeyPair { keypair }
            }
            PayloadType::SymmetricKey => Secret::SymmetricKey {
                key: Box::new(SymmetricKeyInMemory::new(
                    &metadata,
                    KeyMaterial::new(Some(secret_data), None)?,
                )),
            },
        };

        Ok(secret)
    }
}
