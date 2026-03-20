/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::{crypto_trait::Crypto, key_material::KeyMaterial},
    errors::error::VaultError,
    memory::protected_memory::ProtectedMemory,
    types::{metadata::Metadata, secret::KeyPair, secret_id::SecretId},
};

pub struct KeyPairInMemory {
    metadata: Metadata,
    key_material: KeyMaterial,
    crypto: Box<dyn Crypto>,
}

impl KeyPairInMemory {
    pub fn new(metadata: &Metadata, key_material: KeyMaterial, crypto: Box<dyn Crypto>) -> Self {
        Self { metadata: metadata.clone(), key_material, crypto }
    }
}

#[async_trait::async_trait]
impl KeyPair for KeyPairInMemory {
    fn id(&self) -> Option<&SecretId> {
        self.metadata.secret_id.as_ref()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn extractable(&self) -> anyhow::Result<bool> {
        Ok(self.metadata.extractable)
    }

    async fn public_key(&self) -> anyhow::Result<Option<Vec<u8>>> {
        Ok(self.key_material.public_key.clone())
    }

    async fn private_key(&self) -> anyhow::Result<ProtectedMemory> {
        if !self.metadata.extractable {
            anyhow::bail!(VaultError::not_extractable(self.metadata.secret_id.as_ref()))
        }

        let secret_key = self
            .key_material
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

        secret_key.clone().await
    }

    async fn sign(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.crypto.sign(&self.key_material, data, self.metadata.algorithm).await
    }

    async fn verify(&self, data: &[u8], signature: &[u8]) -> anyhow::Result<()> {
        let pub_key = self.key_material.public_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!(VaultError::empty_public_key("Failed to verify signature"))
        })?;

        self.crypto.verify(pub_key, data, signature, self.metadata.algorithm).await
    }

    async fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        self.key_material.serialize().await
    }
}
