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
        crypto_trait::Crypto, factory::CryptoFactory, key_material::KeyMaterial,
        key_pair_in_memory::KeyPairInMemory,
    },
    errors::error::VaultError,
    make_secret_id,
    storage::{file_json::FileJsonStorage, storage_trait::Storage},
    tests::fixture::*,
    types::{
        algorithm::Algorithm, metadata::Metadata, secret::Secret, secret_id::SecretId,
        store_mode::StoreMode,
    },
};
use std::path::PathBuf;

struct V1Vault {
    file_path: PathBuf,
    _temp_dir: tempfile::TempDir,
}

async fn add_ed25519_key_to_vault(
    secret_id: SecretId,
    crypto: Box<dyn Crypto>,
    storage: &FileJsonStorage,
    is_corrupt: bool,
) -> anyhow::Result<()> {
    // Create a 64-byte "expanded" private key (simulating the old bug)
    let mut key_material =
        KeyMaterial::generate_new(Algorithm::Ed25519, None, crypto.as_ref()).await?;

    if is_corrupt {
        key_material
            .secret_key
            .as_mut()
            .ok_or_else(|| VaultError::empty_secret_key(""))?
            .lock_mut()
            .await?
            .extend_from_slice(key_material.public_key.as_ref().unwrap())?;
    }

    // Build a Secret::KeyPair with the 64-byte secret key
    let metadata = Metadata::new(Some(&secret_id), Algorithm::Ed25519, true);
    let keypair = KeyPairInMemory::new(&metadata, key_material, crypto);
    let secret = Secret::KeyPair { keypair: Box::new(keypair) };

    // Store the secret via real FileJsonStorage
    storage.store(&secret, StoreMode::NewOnly).await?;

    Ok(())
}

async fn create_and_fill_v1_vault(
    crypto_factory: Box<dyn CryptoFactory>,
) -> anyhow::Result<V1Vault> {
    let crypto1 = crypto_factory.new_crypto()?;
    let crypto2 = crypto_factory.new_crypto()?;

    let master_key = create_test_master_key().await?;
    let temp_dir = tempfile::TempDir::new()?;
    let file_path = temp_dir.path().join("vault.json");
    let storage = FileJsonStorage::new(master_key, &file_path, crypto_factory, false).await?;

    add_ed25519_key_to_vault(make_secret_id!("Secret_1"), crypto1, &storage, true).await?;
    add_ed25519_key_to_vault(make_secret_id!("Secret_2"), crypto2, &storage, false).await?;
    storage.flush().await?;

    // Patch the on-disk file: set version to 1 to simulate old format
    let json_str = tokio::fs::read_to_string(&file_path).await?;
    let mut json_value: serde_json::Value = serde_json::from_str(&json_str)?;
    json_value["version"] = serde_json::json!(1);
    let patched = serde_json::to_string_pretty(&json_value)?;
    tokio::fs::write(&file_path, &patched).await?;

    Ok(V1Vault { file_path, _temp_dir: temp_dir })
}

#[tokio::test]
#[serial_test::serial]
async fn test_migrate_from_v1() -> anyhow::Result<()> {
    for config in fixture() {
        if config.storage_type != StorageType::FileJson {
            continue;
        }

        let master_key = create_test_master_key().await?;
        let vault_v1_into =
            create_and_fill_v1_vault(create_crypto_factory(config.crypto_type).await?).await?;

        // Run migration
        let storage = FileJsonStorage::new(
            master_key,
            &vault_v1_into.file_path,
            create_crypto_factory(config.crypto_type).await?,
            true,
        )
        .await?;

        // Decrypt the migrated secret and verify the key is now 32 bytes
        let meta_1 = storage
            .load_metadata(&make_secret_id!("Secret_1"))
            .await?
            .ok_or_else(|| VaultError::empty_metadata(""))?;
        let secret_1 = storage.load(&make_secret_id!("Secret_1")).await?;

        let meta_2 = storage
            .load_metadata(&make_secret_id!("Secret_2"))
            .await?
            .ok_or_else(|| VaultError::empty_metadata(""))?;
        let secret_2 = storage.load(&make_secret_id!("Secret_2")).await?;

        assert_eq!(meta_1.algorithm, Algorithm::Ed25519);
        assert_eq!(meta_2.algorithm, Algorithm::Ed25519);

        let key_pair_1 = secret_1.as_keypair()?;
        let key_pair_2 = secret_2.as_keypair()?;

        let pub_key_1 = key_pair_1.public_key().await?.unwrap();
        let pub_key_2 = key_pair_2.public_key().await?.unwrap();

        let pvt_key_1 = key_pair_1.private_key().await?.lock().await?.to_vec();
        let pvt_key_2 = key_pair_2.private_key().await?.lock().await?.to_vec();

        assert_eq!(pub_key_1.len(), 32, "Public key should be 32 bytes after migration");
        assert_eq!(pub_key_2.len(), 32, "Public key should be 32 bytes after migration");
        assert_eq!(pvt_key_1.len(), 32, "Private key should be 32 bytes after migration");
        assert_eq!(pvt_key_2.len(), 32, "Private key should be 32 bytes after migration");

        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;

        assert_eq!(crypto.pub_key_from_pvt(Algorithm::Ed25519, &pvt_key_1).await?, pub_key_1);
        assert_eq!(crypto.pub_key_from_pvt(Algorithm::Ed25519, &pvt_key_2).await?, pub_key_2);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_auto_migrate_false_rejects_v1() -> anyhow::Result<()> {
    for config in fixture() {
        if config.storage_type != StorageType::FileJson {
            continue;
        }

        let master_key = create_test_master_key().await?;
        let vault_v1 =
            create_and_fill_v1_vault(create_crypto_factory(config.crypto_type).await?).await?;

        let result = FileJsonStorage::new(
            master_key,
            &vault_v1.file_path,
            create_crypto_factory(config.crypto_type).await?,
            false,
        )
        .await;

        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(
            err_msg.contains("Wrong vault format version"),
            "Expected format version error, got: {}",
            err_msg
        );
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_migrate_creates_backup() -> anyhow::Result<()> {
    for config in fixture() {
        if config.storage_type != StorageType::FileJson {
            continue;
        }

        let master_key = create_test_master_key().await?;
        let vault_v1 =
            create_and_fill_v1_vault(create_crypto_factory(config.crypto_type).await?).await?;

        let original_content = tokio::fs::read_to_string(&vault_v1.file_path).await?;

        // Run migration
        FileJsonStorage::migrate(
            &vault_v1.file_path,
            master_key.key_material(),
            create_crypto_factory(config.crypto_type).await?.new_crypto()?.as_ref(),
        )
        .await?;

        // Verify a backup file was created
        let dir = vault_v1.file_path.parent().unwrap();
        let mut backup_found = false;
        let mut backup_content = String::new();
        let mut entries = tokio::fs::read_dir(dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.contains("backup_v1") {
                backup_found = true;
                backup_content = tokio::fs::read_to_string(entry.path()).await?;
                break;
            }
        }

        assert!(backup_found, "Backup file should be created during migration");
        assert_eq!(
            backup_content, original_content,
            "Backup should contain the original v1 content"
        );
    }

    Ok(())
}
