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
    memory::protected_memory::ProtectedMemory,
    types::{algorithm::Algorithm, metadata::Metadata, payload::PayloadType, secret_id::SecretId},
};
use core::fmt;

#[async_trait::async_trait]
pub trait Blob: Send + Sync + downcast_rs::Downcast {
    fn id(&self) -> Option<&SecretId>;
    fn metadata(&self) -> &Metadata;
    async fn data(&self) -> anyhow::Result<ProtectedMemory>;
    async fn eq_blob(&self, other: &dyn Blob) -> anyhow::Result<bool> {
        Ok((self.metadata() == other.metadata())
            && self.data().await?.eq_pm(&other.data().await?).await?)
    }
}

downcast_rs::impl_downcast!(Blob);

#[async_trait::async_trait]
pub trait KeyPair: Send + Sync + downcast_rs::Downcast {
    fn id(&self) -> Option<&SecretId>;
    fn metadata(&self) -> &Metadata;
    async fn extractable(&self) -> anyhow::Result<bool>;
    async fn public_key(&self) -> anyhow::Result<Option<Vec<u8>>>;
    async fn private_key(&self) -> anyhow::Result<ProtectedMemory>;
    async fn sign(&self, data: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn verify(&self, data: &[u8], signature: &[u8]) -> anyhow::Result<()>;
    async fn serialize(&self) -> anyhow::Result<ProtectedMemory>;
    async fn eq_keypair(&self, other: &dyn KeyPair) -> anyhow::Result<bool> {
        let extractable = self.extractable().await?;

        let mut is_eq = (extractable == other.extractable().await?)
            && (self.metadata() == other.metadata())
            && (self.public_key().await? == other.public_key().await?);

        if is_eq && extractable {
            is_eq &= self.private_key().await?.eq_pm(&other.private_key().await?).await?;
        }

        Ok(is_eq)
    }
}

downcast_rs::impl_downcast!(KeyPair);

#[async_trait::async_trait]
pub trait SymmetricKey: Send + Sync + downcast_rs::Downcast {
    fn id(&self) -> Option<&SecretId>;
    fn metadata(&self) -> &Metadata;
    async fn extractable(&self) -> anyhow::Result<bool>;
    async fn key(&self) -> anyhow::Result<ProtectedMemory>;
    async fn encrypt(&self, plaintext: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn decrypt(&self, ciphertext: &[u8]) -> anyhow::Result<Vec<u8>>;
    async fn serialize(&self) -> anyhow::Result<ProtectedMemory>;
    async fn eq_key(&self, other: &dyn SymmetricKey) -> anyhow::Result<bool> {
        let extractable = self.extractable().await?;

        let mut is_eq =
            (self.metadata() == other.metadata()) && (extractable == other.extractable().await?);

        if extractable {
            is_eq &= self.key().await?.eq_pm(&other.key().await?).await?;
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
    pub async fn deserialize(
        data: ProtectedMemory,
        metadata: Metadata,
        crypto: Box<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        let secret = match metadata.algorithm.payload_type() {
            PayloadType::Blob => {
                Secret::Blob { blob: Box::new(BlobInMemory::new(&metadata, data)) }
            }
            PayloadType::KeyPair => Secret::KeyPair {
                keypair: Box::new(KeyPairInMemory::new(
                    &metadata,
                    KeyMaterial::deserialize(&data.lock().await?).await?,
                    crypto,
                )),
            },
            PayloadType::SymmetricKey => {
                let key_material = KeyMaterial::deserialize(&data.lock().await?).await?;
                let secret = Secret::SymmetricKey {
                    key: Box::new(SymmetricKeyInMemory::new(&metadata, key_material)),
                };

                return Ok(secret);
            }
        };

        Ok(secret)
    }

    pub async fn from_raw_data(
        secret_data: &[u8],
        metadata: Metadata,
        crypto: Box<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        let secret_data_pm = ProtectedMemory::from_slice(secret_data).await?;
        Self::from_protected_data(secret_data_pm, metadata, crypto).await
    }

    pub async fn from_protected_data(
        secret_data: ProtectedMemory,
        metadata: Metadata,
        crypto: Box<dyn Crypto>,
    ) -> anyhow::Result<Secret> {
        let secret = match metadata.algorithm.payload_type() {
            PayloadType::Blob => {
                Secret::Blob { blob: Box::new(BlobInMemory::new(&metadata, secret_data)) }
            }
            PayloadType::KeyPair => {
                let pub_key = if metadata.algorithm == Algorithm::Ed25519 {
                    let lock = secret_data.lock().await?;
                    crypto.pub_key_from_pvt(Algorithm::Ed25519, &lock).await?
                } else {
                    anyhow::bail!(VaultError::unsupported_algorithm(metadata.algorithm));
                };

                Secret::KeyPair {
                    keypair: Box::new(KeyPairInMemory::new(
                        &metadata,
                        KeyMaterial::new(Some(secret_data), Some(pub_key)).await?,
                        crypto,
                    )),
                }
            }
            PayloadType::SymmetricKey => Secret::SymmetricKey {
                key: Box::new(SymmetricKeyInMemory::new(
                    &metadata,
                    KeyMaterial::new(Some(secret_data), None).await?,
                )),
            },
        };

        Ok(secret)
    }

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

    pub async fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        match self {
            Secret::Blob { blob } => blob.data().await,
            Secret::KeyPair { keypair } => keypair.serialize().await,
            Secret::SymmetricKey { key } => key.serialize().await,
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

    pub async fn eq_secret(&self, other: &Self) -> anyhow::Result<bool> {
        match (self, other) {
            (Secret::Blob { blob: a }, Secret::Blob { blob: b }) => a.eq_blob(b.as_ref()).await,
            (Secret::KeyPair { keypair: a }, Secret::KeyPair { keypair: b }) => {
                a.eq_keypair(b.as_ref()).await
            }
            (Secret::SymmetricKey { key: a }, Secret::SymmetricKey { key: b }) => {
                a.eq_key(b.as_ref()).await
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
