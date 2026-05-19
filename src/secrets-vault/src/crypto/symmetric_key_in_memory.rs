/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use crate::{
    crypto::key_material::KeyMaterial,
    errors::error::VaultError,
    memory::protected_memory::ProtectedMemory,
    types::{metadata::Metadata, secret::SymmetricKey, secret_id::SecretId},
};

pub struct SymmetricKeyInMemory {
    metadata: Metadata,
    key_material: KeyMaterial,
}

impl SymmetricKeyInMemory {
    pub fn new(metadata: &Metadata, key_material: KeyMaterial) -> Self {
        Self { metadata: metadata.clone(), key_material }
    }
}

#[async_trait::async_trait]
impl SymmetricKey for SymmetricKeyInMemory {
    fn id(&self) -> Option<&SecretId> {
        self.metadata.secret_id.as_ref()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }

    fn extractable(&self) -> anyhow::Result<bool> {
        Ok(self.metadata.extractable)
    }

    fn key(&self) -> anyhow::Result<&ProtectedMemory> {
        if !self.metadata.extractable {
            anyhow::bail!(VaultError::not_extractable(self.metadata.secret_id.as_ref()))
        }

        let secret_key = self
            .key_material
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

        Ok(secret_key)
    }

    async fn encrypt(&self, _plaintext: &[u8]) -> anyhow::Result<Vec<u8>> {
        todo!();
    }

    async fn decrypt(&self, _ciphertext: &[u8]) -> anyhow::Result<Vec<u8>> {
        todo!();
    }

    fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        self.key_material.serialize()
    }
}
