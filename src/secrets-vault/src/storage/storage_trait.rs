/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::types::{
    metadata::Metadata, secret::Secret, secret_id::SecretId, secret_spec::SecretSpec,
    store_mode::StoreMode,
};

#[async_trait::async_trait]
pub trait Storage: Send + Sync + downcast_rs::Downcast {
    async fn flush(&self) -> anyhow::Result<()>;
    async fn generate_secret(
        &self,
        spec: &SecretSpec,
        secret_id: &SecretId,
    ) -> anyhow::Result<Secret>;
    async fn store(&self, secret: &Secret, mode: StoreMode) -> anyhow::Result<()>;
    async fn load(&self, secret_id: &SecretId) -> anyhow::Result<Secret>;
    async fn load_metadata(&self, secret_id: &SecretId) -> anyhow::Result<Option<Metadata>>;
    async fn list_metadata(&self) -> anyhow::Result<Vec<Metadata>>;
    async fn delete(&self, secret_id: &SecretId) -> anyhow::Result<()>;
    fn format_version(&self) -> anyhow::Result<u32>;

    #[cfg(test)]
    async fn clear(&self) -> anyhow::Result<()>;

    #[cfg(test)]
    async fn is_empty(&self) -> anyhow::Result<bool>;
}

downcast_rs::impl_downcast!(Storage);
