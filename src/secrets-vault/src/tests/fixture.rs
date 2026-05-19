/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::memory::protected_memory::ProtectedMemoryInner;
#[cfg(feature = "hashicorp-storage")]
use crate::{
    crypto::{
        crypto_trait::Crypto,
        factory::{CryptoFactory, DefaultCryptoFactory},
        key_material::KeyMaterial,
        master_key::MasterKey,
    },
    errors::error::VaultError,
    storage::storage_trait::{ListMode, Storage},
    types::{
        algorithm::Algorithm,
        metadata::Metadata,
        secret::{Secret, SecretDataType, SecretInMemoryFactory},
    },
    vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use core::fmt;
use rand::RngCore;
use std::{path::PathBuf, sync::Arc};

#[allow(dead_code)]
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum StorageType {
    #[cfg(feature = "file-storage-json")]
    FileJson,
    #[cfg(feature = "hashicorp-storage")]
    HashicorpNoCache,
    #[cfg(feature = "hashicorp-storage")]
    HashicorpUseCache,
}

impl StorageType {
    pub fn variant_name(&self) -> &'static str {
        match self {
            #[cfg(feature = "file-storage-json")]
            StorageType::FileJson => "FileJson",
            #[cfg(feature = "hashicorp-storage")]
            StorageType::HashicorpNoCache => "HashicorpNoCache",
            #[cfg(feature = "hashicorp-storage")]
            StorageType::HashicorpUseCache => "HashicorpUseCache",
        }
    }
}

impl fmt::Display for StorageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.variant_name())
    }
}

pub struct TestConfig {
    pub storage_type: StorageType,
}

pub async fn create_test_master_key() -> anyhow::Result<MasterKey> {
    let mut key_data = ProtectedMemoryInner::new(32).unwrap();
    {
        let mut handle = key_data.write_handle()?;
        for i in 0..32 {
            handle[i] = i as u8;
        }
    }

    MasterKey::from_key_material(KeyMaterial::new_symmetric_key(key_data.into())?)
}

pub async fn create_secret(
    secret_data: &[u8],
    secret_data_type: SecretDataType,
    secret_id: &str,
    algorithm: Algorithm,
    crypto: Arc<dyn Crypto>,
) -> anyhow::Result<Secret> {
    create_secret_extractable(secret_data, secret_data_type, secret_id, algorithm, true, crypto)
        .await
}

pub async fn create_secret_extractable(
    secret_data: &[u8],
    secret_data_type: SecretDataType,
    secret_id: &str,
    algorithm: Algorithm,
    extractable: bool,
    crypto: Arc<dyn Crypto>,
) -> anyhow::Result<Secret> {
    let secret_id = secret_id.into();
    let metadata = Metadata::new(Some(&secret_id), algorithm, extractable);
    let secret =
        SecretInMemoryFactory::from_raw_data(secret_data, secret_data_type, metadata, crypto)?;

    Ok(secret)
}

pub async fn create_storage(
    storage_type: StorageType,
    path_opt: Option<PathBuf>,
) -> anyhow::Result<Arc<dyn Storage>> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    let storage: Arc<dyn Storage> = match storage_type {
        #[cfg(feature = "file-storage-json")]
        StorageType::FileJson => {
            use crate::storage::file_json::FileJsonStorage;

            let file_path = match path_opt {
                Some(p) => p,
                None => {
                    let temp_dir = tempfile::TempDir::new()?;
                    let p = temp_dir.path().join("secrets.json");
                    // Leak the temp_dir to keep it alive
                    std::mem::forget(temp_dir);
                    p
                }
            };
            let master_key = create_test_master_key().await?;
            let storage = FileJsonStorage::new(master_key, &file_path, false, crypto).await?;

            Arc::new(storage)
        }
        #[cfg(feature = "hashicorp-storage")]
        StorageType::HashicorpNoCache => {
            // NOTE: HashiCorp Vault must be launched in dev mode on http://127.0.0.1:8200/ (./vault server -dev -dev-root-token-id=root)
            use crate::{
                memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
                storage::{
                    hashicorp::HashicorpStorage, hashicorp_api::VaultConfig,
                    hashicorp_token_provider::AuthConfig,
                },
            };

            let api_key_data: ProtectedMemory =
                ProtectedMemoryInner::from_slice("root".as_bytes())?.into();
            let storage = HashicorpStorage::new(
                AuthConfig::StaticToken(api_key_data),
                "http://127.0.0.1:8200",
                false,
                crypto,
                VaultConfig::default(),
            )?;
            Arc::new(storage)
        }
        #[cfg(feature = "hashicorp-storage")]
        StorageType::HashicorpUseCache => {
            // NOTE: HashiCorp Vault must be launched in dev mode on http://127.0.0.1:8200/ (./vault server -dev -dev-root-token-id=root)
            use crate::{
                memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
                storage::{
                    hashicorp::HashicorpStorage, hashicorp_api::VaultConfig,
                    hashicorp_token_provider::AuthConfig,
                },
            };

            let api_key_data: ProtectedMemory =
                ProtectedMemoryInner::from_slice("root".as_bytes())?.into();
            let storage = HashicorpStorage::new(
                AuthConfig::StaticToken(api_key_data),
                "http://127.0.0.1:8200",
                true,
                crypto,
                VaultConfig::default(),
            )?;
            Arc::new(storage)
        }
    };

    Ok(storage)
}

pub fn create_url(
    storage_type: StorageType,
    modify_url: Option<impl FnOnce(String) -> String>,
) -> anyhow::Result<(String, Option<tempfile::TempDir>)> {
    let (url, temp_dir) = match storage_type {
        #[cfg(feature = "file-storage-json")]
        StorageType::FileJson => {
            let temp_dir = tempfile::tempdir()?;
            let random_name = rand::random::<u64>().to_string();
            let file_path = temp_dir.path().join(format!("{random_name}.json"));
            let file_path_str =
                file_path.to_str().ok_or_else(|| anyhow::anyhow!("Failed to make path"))?;
            let url = format!(
                "file://{file_path_str}?master_key=abcdef00000000000011223344556677889900112233445566778899001122ff"
            );
            (url, Some(temp_dir))
        }
        #[cfg(feature = "hashicorp-storage")]
        StorageType::HashicorpNoCache | StorageType::HashicorpUseCache => {
            ("hashicorp://http://127.0.0.1:8200?api_key=root".to_string(), None)
        }
    };

    let url = match modify_url {
        Some(f) => f(url),
        None => url,
    };

    Ok((url, temp_dir))
}

pub async fn create_test_storage(config: &TestConfig) -> anyhow::Result<Arc<dyn Storage>> {
    let storage = create_storage(config.storage_type, None).await?;
    clear_storage(storage.as_ref()).await?;
    Ok(storage)
}

pub async fn create_test_vault(config: &TestConfig) -> anyhow::Result<SecretVault> {
    let storage = create_test_storage(config).await?;
    let vault = SecretVaultBuilder::default().with_storage(storage).build().await?;

    Ok(vault)
}

pub fn make_ed25519_test_key_32() -> [u8; 32] {
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    key
}

pub async fn clear_storage(storage: &dyn Storage) -> anyhow::Result<()> {
    let metas = storage.list_metadata(ListMode::All).await?;

    for meta in &metas {
        let secret_id = meta.secret_id.as_ref().ok_or_else(|| VaultError::empty_secret_id(""))?;
        storage.delete(secret_id).await?;
    }

    Ok(())
}

pub async fn clear_vault(vault: &SecretVault) -> anyhow::Result<()> {
    let metas = vault.list_metadata(ListMode::All).await?;

    for meta in &metas {
        let secret_id = meta.secret_id.as_ref().ok_or_else(|| VaultError::empty_secret_id(""))?;
        vault.delete(secret_id).await?;
    }

    Ok(())
}

pub fn fixture() -> Vec<TestConfig> {
    let configs = vec![
        //File storage (json) + default cryptography
        #[cfg(all(feature = "crypto-default", feature = "file-storage-json"))]
        TestConfig { storage_type: StorageType::FileJson },
        // Hashicorp Vault
        // TODO: setup vault in CI/CD test
        //#[cfg(all(feature = "crypto-default", feature = "hashicorp-storage"))]
        //TestConfig { storage_type: StorageType::HashicorpNoCache },
        //#[cfg(all(feature = "crypto-default", feature = "hashicorp-storage"))]
        //TestConfig { storage_type: StorageType::HashicorpUseCache },
    ];

    configs
}
