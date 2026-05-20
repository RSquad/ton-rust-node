/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    errors::error::VaultError,
    events::{
        event_types::{Event, EventType},
        handler::EventHandler,
    },
    storage::storage_trait::Storage,
    types::{
        metadata::Metadata, secret::Secret, secret_id::SecretId, secret_spec::SecretSpec,
        store_mode::StoreMode,
    },
};
use std::sync::Arc;

pub struct SecretVault {
    storage: Arc<dyn Storage>,
    event_handler: Arc<dyn EventHandler>,
}

impl SecretVault {
    pub fn new(storage: Arc<dyn Storage>, event_handler: Arc<dyn EventHandler>) -> Self {
        SecretVault { storage, event_handler }
    }

    pub fn storage(&self) -> &Arc<dyn Storage> {
        &self.storage
    }

    pub async fn flush(&self) -> anyhow::Result<()> {
        self.storage.flush().await
    }

    pub async fn generate_secret(
        &self,
        spec: &SecretSpec,
        secret_id: &SecretId,
    ) -> anyhow::Result<Secret> {
        let secret = self.storage.generate_secret(spec, secret_id).await?;

        // Event
        let event = Event::new(EventType::KeyGenerated)
            .with_secret_id(secret_id)
            .with_algorithm(spec.algorithm);
        let _ = self.event_handler.put(event).await;

        Ok(secret)
    }

    pub async fn load(&self, secret_id: &SecretId) -> anyhow::Result<Secret> {
        self.storage.load(secret_id).await
    }

    pub async fn exists(&self, secret_id: &SecretId) -> anyhow::Result<bool> {
        match self.storage.load_metadata(secret_id).await {
            Ok(Some(_)) => Ok(true),
            Ok(None) => Ok(false),
            Err(e) => {
                if e.downcast_ref::<VaultError>()
                    .is_some_and(|ve| ve.code() == VaultError::NOT_FOUND)
                {
                    Ok(false)
                } else {
                    Err(e)
                }
            }
        }
    }

    pub async fn store(&self, secret: &Secret, mode: StoreMode) -> anyhow::Result<()> {
        self.storage.store(secret, mode, None).await
    }

    pub async fn delete(&self, secret_id: &SecretId) -> anyhow::Result<()> {
        self.storage.delete(secret_id).await
    }

    pub async fn load_metadata(&self, secret_id: &SecretId) -> anyhow::Result<Metadata> {
        let meta = self.storage.load_metadata(secret_id).await?.ok_or_else(|| {
            VaultError::not_found(format!("Metadata not found for secret id '{}'", secret_id))
        })?;

        Ok(meta)
    }

    pub async fn list_metadata(&self) -> anyhow::Result<Vec<Metadata>> {
        self.storage.list_metadata().await
    }

    #[cfg(test)]
    pub async fn clear(&self) -> anyhow::Result<()> {
        self.storage.clear().await
    }
}

impl std::fmt::Debug for SecretVault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretVault")
            .field("storage", &"<dyn Storage>")
            .field("event_handler", &"<dyn EventHandler>")
            .finish()
    }
}
