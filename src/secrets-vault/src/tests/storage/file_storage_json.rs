/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    storage::file_json::FileJsonStorage,
    tests::fixture::*,
    types::{algorithm::Algorithm, metadata::Metadata, secret::Secret, store_mode::StoreMode},
};

#[tokio::test]
#[serial_test::serial]
async fn test_file_format_is_json() -> anyhow::Result<()> {
    for config in fixture() {
        if config.storage_type != StorageType::FileJson {
            continue;
        }

        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await.unwrap();
        let secret_id = "test/key".into();
        let metadata = Metadata::new(Some(&secret_id), Algorithm::Aes256Gcm, true);
        let secret =
            Secret::from_raw_data(b"test_value", metadata, crypto_factory.new_crypto()?).await?;

        storage.store(&secret, StoreMode::NewOnly).await?;

        let file_storage = storage.as_ref().downcast_ref::<FileJsonStorage>().unwrap();
        let file_content = tokio::fs::read_to_string(file_storage.file_path()).await?;
        let parsed: serde_json::Value = serde_json::from_str(&file_content)?;

        assert!(parsed.get("version").is_some());
        assert!(parsed.get("tree").is_some());
    }

    Ok(())
}
