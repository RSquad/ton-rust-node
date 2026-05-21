/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::factory::{CryptoFactory, DefaultCryptoFactory},
    storage::file_json::FileJsonStorage,
    tests::fixture::*,
    types::{
        algorithm::Algorithm, metadata::Metadata, secret::SecretInMemoryFactory,
        store_mode::StoreMode,
    },
};

#[tokio::test]
#[serial_test::serial]
async fn test_file_format_is_json() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    for config in fixture() {
        if config.storage_type != StorageType::FileJson {
            continue;
        }

        let storage = create_test_storage(&config).await.unwrap();
        let secret_id = "test/key".into();
        let metadata = Metadata::new(Some(&secret_id), Algorithm::Aes256Gcm, true);
        let secret = SecretInMemoryFactory::new_secret(b"test_value", metadata, crypto.clone())?;

        storage.store(&secret, StoreMode::NewOnly, None).await?;

        let file_storage = storage.as_ref().downcast_ref::<FileJsonStorage>().unwrap();
        let file_content = tokio::fs::read_to_string(file_storage.file_path()).await?;
        let parsed: serde_json::Value = serde_json::from_str(&file_content)?;

        assert!(parsed.get("version").is_some());
        assert!(parsed.get("tree").is_some());
    }

    Ok(())
}
