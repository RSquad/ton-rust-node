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
use std::sync::Arc;

pub struct KeyPairInMemory {
    metadata: Metadata,
    key_material: KeyMaterial,
    crypto: Arc<dyn Crypto>,
}

impl KeyPairInMemory {
    pub fn new(metadata: &Metadata, key_material: KeyMaterial, crypto: Arc<dyn Crypto>) -> Self {
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

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }

    fn extractable(&self) -> bool {
        self.metadata.extractable
    }

    fn public_key(&self) -> Option<&[u8]> {
        self.key_material.public_key.as_deref()
    }

    fn private_key(&self) -> anyhow::Result<&ProtectedMemory> {
        if !self.metadata.extractable {
            anyhow::bail!(VaultError::not_extractable(self.metadata.secret_id.as_ref()))
        }

        let pvt_key = self
            .key_material
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Private key is not set"))?;

        Ok(pvt_key)
    }

    fn expanded_key(&self) -> anyhow::Result<ProtectedMemory> {
        let pvt_key = self.private_key()?;

        let crypto = self.crypto.as_ref();
        let lock = &pvt_key.lock()?;
        let exp_key = crypto.exp_key_from_pvt(self.metadata.algorithm, lock)?;

        Ok(exp_key)
    }

    async fn sign(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        self.crypto.sign(&self.key_material, data, self.metadata.algorithm)
    }

    async fn verify(&self, data: &[u8], signature: &[u8]) -> anyhow::Result<()> {
        let pub_key = self.key_material.public_key.as_ref().ok_or_else(|| {
            anyhow::anyhow!(VaultError::empty_public_key("Failed to verify signature"))
        })?;

        self.crypto.verify(pub_key, data, signature, self.metadata.algorithm)
    }

    fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        self.key_material.serialize()
    }
}
