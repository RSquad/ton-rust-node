/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "file-storage-json")]
use crate::storage::file_json::FileJsonStorage;
#[cfg(feature = "hashicorp-storage")]
use crate::storage::hashicorp::HashicorpStorage;
use crate::{
    crypto::{key_material::KeyMaterial, master_key::MasterKey},
    errors::error::VaultError,
    events::{handler::EventHandler, null_handler::NullEventHandler},
    storage::storage_trait::Storage,
    vault::SecretVault,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct VaultUrl<'a> {
    pub storage_name: String,
    pub path: &'a str,
    query: Option<&'a str>,
}

impl<'a> VaultUrl<'a> {
    pub fn parse(url: &'a str) -> anyhow::Result<Self> {
        let input = url.trim();
        let (storage_name_raw, rest) = input
            .split_once("://")
            .ok_or_else(|| VaultError::invalid_config_url("No delimiter found '://'"))?;
        let storage_name = storage_name_raw.trim().to_ascii_lowercase();

        let mut split = rest.splitn(2, '?');
        let path = split.next().unwrap_or("").trim();
        let query = split.next();

        if let Some(q) = query {
            for seg in q.split('&') {
                let seg = seg.trim();
                if !seg.is_empty() && !seg.contains('=') {
                    anyhow::bail!(VaultError::invalid_config_url(format!(
                        "malformed query parameter: '{seg}'"
                    )));
                }
            }
        }

        Ok(Self { storage_name, path, query })
    }

    pub fn query_param(&self, name: &str) -> Option<&'a str> {
        let q = self.query?;
        for seg in q.split('&') {
            let seg = seg.trim();
            if seg.is_empty() {
                continue;
            }
            if let Some((k, v)) = seg.split_once('=') {
                if k.trim().eq_ignore_ascii_case(name) {
                    return Some(v.trim());
                }
            }
        }
        None
    }
}

#[derive(Default)]
pub struct SecretVaultBuilder {
    storage: Option<Arc<dyn Storage>>,
    event_handler: Option<Arc<dyn EventHandler>>,
}

impl SecretVaultBuilder {
    pub async fn from_url_or_env(url: Option<&str>) -> anyhow::Result<Option<Arc<SecretVault>>> {
        let url_from_env = Self::read_url_from_env()?;

        if url_from_env.is_some() && url.is_some() {
            anyhow::bail!(VaultError::invalid_config_url("Set vault config either in a file or in the ENV variable VAULT_URL, but not both. Currently, both are set."))
        }

        let Some(url) = url.or(url_from_env.as_deref()) else {
            return Ok(None);
        };

        let vault = SecretVaultBuilder::from_url(url).await?;

        Ok(Some(vault))
    }

    pub async fn from_env() -> anyhow::Result<Arc<SecretVault>> {
        let vault = Self::from_url_or_env(None).await?.ok_or(VaultError::invalid_config_url(
            "The VAULT_URL environment variable is not set",
        ))?;

        Ok(vault)
    }

    pub async fn from_url(url: &str) -> anyhow::Result<Arc<SecretVault>> {
        let parsed = VaultUrl::parse(url)?;

        if parsed.storage_name == "file" {
            #[cfg(feature = "file-storage-json")]
            {
                return Ok(Arc::new(Self::from_url_file(&parsed).await?.build().await?));
            }
        } else if parsed.storage_name == "hashicorp" {
            #[cfg(feature = "hashicorp-storage")]
            {
                return Ok(Arc::new(Self::from_url_hashicorp(&parsed).await?.build().await?));
            }
        }

        anyhow::bail!(VaultError::invalid_config_url(format!(
            "unsupported storage '{}'",
            parsed.storage_name
        )));
    }

    #[cfg(feature = "hashicorp-storage")]
    async fn from_url_hashicorp(parsed: &VaultUrl<'_>) -> anyhow::Result<Self> {
        use crate::{crypto::factory::AutoCryptoFactory, utils::hex::val_to_pm};

        if parsed.path.is_empty() {
            anyhow::bail!(VaultError::invalid_config_url("Missing vault url part"));
        }

        let api_key_val = parsed
            .query_param("api_key")
            .ok_or_else(|| VaultError::invalid_config_url("missing parameter `api_key`"))?;
        let api_key = val_to_pm("api_key", api_key_val).await?;
        let namespace = parsed.query_param("namespace").map(|s| s.to_string());
        let prefer_local_crypto = parsed
            .query_param("prefer_local_crypto")
            .is_some_and(|s| s.eq_ignore_ascii_case("true"));

        let storage = Arc::new(
            HashicorpStorage::new(
                api_key,
                parsed.path,
                namespace.as_deref(),
                Box::new(AutoCryptoFactory {}),
                prefer_local_crypto,
            )
            .await?,
        );
        Ok(Self::default().with_storage(storage))
    }

    #[cfg(feature = "file-storage-json")]
    async fn from_url_file(parsed: &VaultUrl<'_>) -> anyhow::Result<Self> {
        use crate::{crypto::factory::AutoCryptoFactory, utils::hex::hex_val_to_pm};

        if parsed.path.is_empty() {
            anyhow::bail!(VaultError::invalid_config_url("Missing path part"));
        }

        let master_key_val = parsed
            .query_param("master_key")
            .ok_or_else(|| VaultError::invalid_config_url("missing parameter `master_key`"))?;
        let master_key = hex_val_to_pm("master_key", master_key_val).await?;
        let auto_migrate = parsed
            .query_param("auto_migrate")
            .map(|v| v.eq_ignore_ascii_case("true"))
            .unwrap_or(true);

        let master_key_material = KeyMaterial::new_symmetric_key(master_key).await?;

        let path_buf = if Path::new(parsed.path).is_absolute() {
            PathBuf::from(parsed.path)
        } else {
            std::env::current_dir()?.join(parsed.path)
        };

        let master_key = MasterKey::from_key_material(master_key_material).await?;
        let storage = Arc::new(
            FileJsonStorage::new(
                master_key,
                &path_buf,
                Box::new(AutoCryptoFactory {}),
                auto_migrate,
            )
            .await?,
        );
        Ok(Self::default().with_storage(storage))
    }

    pub fn read_url_from_env() -> anyhow::Result<Option<String>> {
        match std::env::var("VAULT_URL") {
            Ok(val) => Ok(Some(val)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub async fn build(self) -> anyhow::Result<SecretVault> {
        if self.storage.is_none() {
            anyhow::bail!(VaultError::storage_not_set("use with_storage"))
        }

        let event_handler: Arc<dyn EventHandler> = match self.event_handler {
            Some(h) => h,
            None => Arc::new(NullEventHandler),
        };

        Ok(SecretVault::new(self.storage.unwrap(), event_handler))
    }

    pub fn with_storage(mut self, storage: Arc<dyn Storage>) -> Self {
        self.storage = Some(storage);
        self
    }

    pub fn with_event_handler(mut self, event_handler: Arc<dyn EventHandler>) -> Self {
        self.event_handler = Some(event_handler);
        self
    }

    pub fn storage(&self) -> &Option<Arc<dyn Storage>> {
        &self.storage
    }

    pub fn event_handler(&self) -> &Option<Arc<dyn EventHandler>> {
        &self.event_handler
    }
}
