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
    make_secret_id,
    tests::fixture::*,
    types::{algorithm::Algorithm, secret::Secret, secret_spec::SecretSpec, store_mode::StoreMode},
};
use rand::RngCore;

#[tokio::test]
#[serial_test::serial]
async fn test_new_storage() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let metadata_list = storage.list_metadata().await?;
        assert!(metadata_list.is_empty());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_store_and_load_new_only() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key_1",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;
        let result = storage.store(&secret, StoreMode::NewOnly).await;
        assert!(result.is_ok());

        let secret_loaded = storage.load(secret.id().unwrap()).await?;
        assert!(secret.eq_secret(&secret_loaded).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_store_replace_if_exists() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        let secret1 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key_1",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        storage.store(&secret1, StoreMode::CreateOrReplace).await?;

        let secret2 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key_1",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let result = storage.store(&secret2, StoreMode::ReplaceExists).await;
        assert!(result.is_ok());

        let secret2_id = secret2.metadata().secret_id.as_ref().unwrap();
        let secret2_loaded = storage.load(secret2_id).await?;

        assert!(&secret2_loaded.eq_secret(&secret2).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_store_create_or_replace_new_key() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret1 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "new_key",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;

        let result = storage.store(&secret1, StoreMode::CreateOrReplace).await;
        let secret1_id = secret1.metadata().secret_id.as_ref().unwrap();
        assert!(result.is_ok());
        assert!(storage.load(secret1_id).await.is_ok());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_store_create_or_replace_existing_key() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        let secret1 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        storage.store(&secret1, StoreMode::CreateOrReplace).await?;

        let secret2 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let result = storage.store(&secret2, StoreMode::CreateOrReplace).await;
        assert!(result.is_ok());

        let secret_id = secret2.metadata().secret_id.as_ref().unwrap();
        let secret2_loaded = storage.load(secret_id).await?;
        assert!(secret2_loaded.eq_secret(&secret2).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_load_nonexistent_key() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let nonexistent_id = "nonexistent".into();
        let result = storage.load(&nonexistent_id).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_load_metadata() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret1 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;

        storage.store(&secret1, StoreMode::NewOnly).await?;
        let secret_id = secret1.metadata().secret_id.as_ref().unwrap();

        let loaded_metadata = storage
            .load_metadata(secret_id)
            .await?
            .ok_or_else(|| VaultError::empty_metadata(""))?;
        assert_eq!(loaded_metadata.secret_id.as_ref().unwrap(), secret_id);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_load_metadata_nonexistent() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let nonexistent_id = "nonexistent".into();
        let result = storage.load_metadata(&nonexistent_id).await?;
        assert!(result.is_none());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_list_metadata_empty() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let metadata_list = storage.list_metadata().await?;
        assert!(metadata_list.is_empty());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_list_metadata_multiple_entries() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        for i in 0..5 {
            let secret = create_secret(
                make_ed25519_test_key_32().as_ref(),
                &format!("key_{}", i),
                Algorithm::Ed25519,
                crypto_factory.new_crypto()?,
            )
            .await?;
            storage.store(&secret, StoreMode::NewOnly).await?;
        }

        let metadata_list = storage.list_metadata().await?;
        assert_eq!(metadata_list.len(), 5);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_delete_existing_key() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;

        storage.store(&secret, StoreMode::NewOnly).await?;

        let secret_id = secret.metadata().secret_id.as_ref().unwrap();
        let result = storage.delete(secret_id).await;
        assert!(result.is_ok());
        assert!(storage.load(secret_id).await.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_delete_nonexistent_key() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let nonexistent_id = "nonexistent".into();
        let result = storage.delete(&nonexistent_id).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_exists_true() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "test_key",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;

        storage.store(&secret, StoreMode::NewOnly).await?;

        let secret_id = secret.metadata().secret_id.as_ref().unwrap();
        assert!(storage.load(secret_id).await.is_ok());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_exists_false() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let nonexistent_id = "nonexistent".into();
        assert!(storage.load(&nonexistent_id).await.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_store_empty_data() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let result = create_secret(b"", "empty_key", Algorithm::Ed25519, crypto).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_multiple_operations_sequence() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;

        let secret1 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "key1",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let secret1_id = secret1.metadata().secret_id.as_ref().unwrap();
        storage.store(&secret1, StoreMode::NewOnly).await?;

        let secret2 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "key2",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let secret2_id = secret2.metadata().secret_id.as_ref().unwrap();
        storage.store(&secret2, StoreMode::NewOnly).await?;

        let secret3 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "key3",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let secret3_id = secret3.metadata().secret_id.as_ref().unwrap();
        storage.store(&secret3, StoreMode::NewOnly).await?;

        assert!(storage.load(secret1_id).await.is_ok());
        assert!(storage.load(secret2_id).await.is_ok());
        assert!(storage.load(secret2_id).await.is_ok());

        let secret2_updated = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "key2",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        storage.store(&secret2_updated, StoreMode::ReplaceExists).await?;

        storage.delete(&secret3_id).await?;

        assert!(storage.load(&secret1_id).await.is_ok());
        assert!(storage.load(&secret2_id).await.is_ok());
        assert!(storage.load(&secret3_id).await.is_err());

        let secret2_loaded = storage.load(secret2_id).await?;
        assert!(secret2_loaded.eq_secret(&secret2_updated).await?);

        let metadata_list = storage.list_metadata().await?;
        assert_eq!(metadata_list.len(), 2);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_concurrent_stores() -> anyhow::Result<()> {
    use std::sync::Arc;

    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        let mut set = tokio::task::JoinSet::new();

        for i in 0..10 {
            let storage = Arc::clone(&storage);
            let crypto = crypto_factory.new_crypto()?;
            set.spawn(async move {
                let secret = create_secret(
                    make_ed25519_test_key_32().as_ref(),
                    &format!("test_concurrent_stores_key_{}", i),
                    Algorithm::Ed25519,
                    crypto,
                )
                .await
                .unwrap();
                storage.store(&secret, StoreMode::CreateOrReplace).await.unwrap();

                secret.metadata().secret_id.clone()
            });
        }

        while let Some(result) = set.join_next().await {
            let secret_id = result?;
            println!("Secret id: {}", secret_id.as_ref().unwrap());
        }

        let metadata_list = storage.list_metadata().await?;

        for m in &metadata_list {
            println!("Secret id: {}", m.secret_id.as_ref().unwrap());
        }

        assert_eq!(metadata_list.len(), 10);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_data_integrity_after_encryption() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "integrity_test",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;
        let secret_id = secret.metadata().secret_id.as_ref().unwrap();

        storage.store(&secret, StoreMode::NewOnly).await?;

        let secret_loaded = storage.load(secret_id).await?;
        assert!(secret_loaded.eq_secret(&secret).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_hierarchical_paths() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        let paths = vec![
            "group1.keys.key_1",
            "group1.keys.key_2",
            "group1.subgroup.key_3",
            "group2.key_4",
            "root_key",
        ];

        for path in &paths {
            let secret = create_secret(
                make_ed25519_test_key_32().as_ref(),
                (*path).into(),
                Algorithm::Ed25519,
                crypto_factory.new_crypto()?,
            )
            .await?;

            storage.store(&secret, StoreMode::NewOnly).await?;
        }

        for path in &paths {
            let secret_id = (*path).into();
            let exists = storage.load(&secret_id).await.is_ok();
            assert!(exists, "Secret '{}' should exist", path);

            let secret_loaded = storage.load(&secret_id).await?;
            assert_eq!(secret_loaded.metadata().secret_id.as_ref().unwrap().as_str(), *path);
        }
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_delete_cleans_empty_nodes() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret: crate::types::secret::Secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "a.b.c.d.key",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;
        let secret_id = secret.metadata().secret_id.as_ref().unwrap();

        storage.store(&secret, StoreMode::NewOnly).await?;
        storage.delete(&secret_id).await?;

        let metadata_list = storage.list_metadata().await?;
        assert_eq!(metadata_list.len(), 0);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_empty_path_segments() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "group1.keys.key_1",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;
        let secret_id = secret.metadata().secret_id.as_ref().unwrap();

        storage.store(&secret, StoreMode::NewOnly).await?;

        let secret_loaded = storage.load(&secret_id).await?;
        assert!(secret_loaded.eq_secret(&secret).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_deep_nested_paths() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "a.b.c.d.e.f.g.h.i.j.k.l.m.n.o.p.q.r.s.t.key",
            Algorithm::Ed25519,
            crypto,
        )
        .await?;
        let secret_id = secret.metadata().secret_id.as_ref().unwrap();

        storage.store(&secret, StoreMode::NewOnly).await?;

        let secret_loaded = storage.load(&secret_id).await?;
        assert!(secret_loaded.eq_secret(&secret).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_update_existing_secret() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "update_test",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let secret_id = secret.metadata().secret_id.as_ref().unwrap();

        storage.store(&secret, StoreMode::NewOnly).await?;

        let secret2 = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "update_test",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;

        storage.store(&secret2, StoreMode::ReplaceExists).await?;

        let secret2_loaded = storage.load(&secret_id).await?;
        assert!(secret2_loaded.eq_secret(&secret2).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_compare_key_size() -> anyhow::Result<()> {
    let secret_id = make_secret_id!("secret");
    let mut base_secret: Option<Secret> = None;

    for config in fixture() {
        let storage = create_test_storage(&config).await?;

        let secret = if let Some(base_secret) = &base_secret {
            storage.store(base_secret, StoreMode::CreateOrReplace).await?;
            storage.load(&secret_id).await?
        } else {
            let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
            let secret = storage.generate_secret(&spec, &secret_id).await?;

            base_secret = Some(secret);
            storage.load(&secret_id).await?
        };

        let keypair = secret.as_keypair()?;
        let base_keypair = base_secret.as_ref().unwrap().as_keypair()?;
        assert!(keypair.eq_keypair(base_keypair).await?);

        assert_eq!(secret.metadata().algorithm, Algorithm::Ed25519);
        assert_eq!(secret.metadata().secret_id.as_ref().unwrap(), &secret_id);

        let public_key = keypair.public_key().await?.unwrap();
        assert_eq!(public_key.len(), 32);

        let private_key = keypair.private_key().await?;
        let private_key_lock = private_key.lock().await?;
        let private_key_data: &[u8] = &private_key_lock;
        assert_eq!(private_key_data.len(), 32);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_compare_signature_extractable() -> anyhow::Result<()> {
    test_compare_signature(true).await
}

async fn test_compare_signature(extractable: bool) -> anyhow::Result<()> {
    let secret_id: crate::types::secret_id::SecretId = make_secret_id!("secret");
    let mut base_secret: Option<Secret> = None;
    let data = "THIS IS SIGNED MESSAGE!";

    for config in fixture() {
        let storage = create_test_storage(&config).await?;

        let secret = if let Some(base_secret) = &base_secret {
            storage.store(base_secret, StoreMode::CreateOrReplace).await?;
            storage.load(&secret_id).await?
        } else {
            let spec = SecretSpec::new(Algorithm::Ed25519).extractable(extractable);
            let secret = storage.generate_secret(&spec, &secret_id).await?;

            base_secret = Some(secret);
            storage.load(&secret_id).await?
        };

        let keypair = secret.as_keypair()?;
        let base_keypair = base_secret.as_ref().unwrap().as_keypair()?;

        // sign
        let base_sign = base_keypair.sign(data.as_bytes()).await?;
        let sign = keypair.sign(data.as_bytes()).await?;
        assert_eq!(&base_sign, &sign);

        // verify
        base_keypair.verify(data.as_bytes(), &base_sign).await?;
        base_keypair.verify(data.as_bytes(), &sign).await?;
        keypair.verify(data.as_bytes(), &sign).await?;
        keypair.verify(data.as_bytes(), &base_sign).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_empty_storage() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        storage.list_metadata().await?;
    }

    Ok(())
}

fn make_blob_test_data(size: usize) -> Vec<u8> {
    let mut data = vec![0u8; size];
    rand::thread_rng().fill_bytes(&mut data);
    data
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_store_and_load_new_only() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let blob_data = make_blob_test_data(64);
        let secret = create_secret(&blob_data, "test_blob_1", Algorithm::None, crypto).await?;

        storage.store(&secret, StoreMode::NewOnly).await?;

        let loaded = storage.load(secret.id().unwrap()).await?;
        assert!(secret.eq_secret(&loaded).await?);
        assert_eq!(loaded.metadata().algorithm, Algorithm::None);
        assert!(loaded.metadata().is_blob());

        let loaded_blob = loaded.as_blob()?;
        let loaded_data = loaded_blob.data().await?;
        let lock = loaded_data.lock().await?;
        assert_eq!(lock.as_ref(), blob_data.as_slice());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_store_replace_if_exists() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;

        let blob1 = create_secret(
            &make_blob_test_data(32),
            "test_blob",
            Algorithm::None,
            crypto_factory.new_crypto()?,
        )
        .await?;
        storage.store(&blob1, StoreMode::CreateOrReplace).await?;

        let blob2_data = make_blob_test_data(32);
        let blob2 =
            create_secret(&blob2_data, "test_blob", Algorithm::None, crypto_factory.new_crypto()?)
                .await?;
        storage.store(&blob2, StoreMode::ReplaceExists).await?;

        let loaded = storage.load(blob2.id().unwrap()).await?;
        assert!(loaded.eq_secret(&blob2).await?);

        let loaded_blob = loaded.as_blob()?;
        let loaded_data = loaded_blob.data().await?;
        let lock = loaded_data.lock().await?;
        assert_eq!(lock.as_ref(), blob2_data.as_slice());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_delete() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret =
            create_secret(&make_blob_test_data(48), "blob_del", Algorithm::None, crypto).await?;
        let secret_id = secret.id().unwrap().clone();

        storage.store(&secret, StoreMode::NewOnly).await?;
        assert!(storage.load(&secret_id).await.is_ok());

        storage.delete(&secret_id).await?;
        assert!(storage.load(&secret_id).await.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_data_integrity_after_encryption() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let blob_data = make_blob_test_data(128);
        let secret = create_secret(&blob_data, "blob_integrity", Algorithm::None, crypto).await?;
        let secret_id = secret.id().unwrap();

        storage.store(&secret, StoreMode::NewOnly).await?;

        let loaded = storage.load(secret_id).await?;
        assert!(loaded.eq_secret(&secret).await?);

        let loaded_blob = loaded.as_blob()?;
        let loaded_data = loaded_blob.data().await?;
        let lock = loaded_data.lock().await?;
        assert_eq!(lock.as_ref(), blob_data.as_slice());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_multiple_operations_sequence() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;

        let blob1 = create_secret(
            &make_blob_test_data(32),
            "blob_1",
            Algorithm::None,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let blob1_id = blob1.id().unwrap().clone();
        storage.store(&blob1, StoreMode::NewOnly).await?;

        let blob2 = create_secret(
            &make_blob_test_data(64),
            "blob_2",
            Algorithm::None,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let blob2_id = blob2.id().unwrap().clone();
        storage.store(&blob2, StoreMode::NewOnly).await?;

        let blob3 = create_secret(
            &make_blob_test_data(48),
            "blob_3",
            Algorithm::None,
            crypto_factory.new_crypto()?,
        )
        .await?;
        let blob3_id = blob3.id().unwrap().clone();
        storage.store(&blob3, StoreMode::NewOnly).await?;

        assert!(storage.load(&blob1_id).await.is_ok());
        assert!(storage.load(&blob2_id).await.is_ok());
        assert!(storage.load(&blob3_id).await.is_ok());

        let blob2_updated_data = make_blob_test_data(64);
        let blob2_updated = create_secret(
            &blob2_updated_data,
            "blob_2",
            Algorithm::None,
            crypto_factory.new_crypto()?,
        )
        .await?;
        storage.store(&blob2_updated, StoreMode::ReplaceExists).await?;

        storage.delete(&blob3_id).await?;

        assert!(storage.load(&blob1_id).await.is_ok());
        assert!(storage.load(&blob3_id).await.is_err());

        let blob2_loaded = storage.load(&blob2_id).await?;
        assert!(blob2_loaded.eq_secret(&blob2_updated).await?);

        let blob2_loaded_data = blob2_loaded.as_blob()?.data().await?;
        let lock = blob2_loaded_data.lock().await?;
        assert_eq!(lock.as_ref(), blob2_updated_data.as_slice());

        let metadata_list = storage.list_metadata().await?;
        assert_eq!(metadata_list.len(), 2);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_different_sizes() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;

        for (i, size) in [1, 16, 32, 64, 128, 256, 1024].iter().enumerate() {
            let blob_data = make_blob_test_data(*size);
            let secret = create_secret(
                &blob_data,
                &format!("blob_size_{}", i),
                Algorithm::None,
                crypto_factory.new_crypto()?,
            )
            .await?;
            let secret_id = secret.id().unwrap().clone();

            storage.store(&secret, StoreMode::NewOnly).await?;

            let loaded = storage.load(&secret_id).await?;
            let loaded_blob = loaded.as_blob()?;
            let loaded_data = loaded_blob.data().await?;
            let lock = loaded_data.lock().await?;
            assert_eq!(lock.len(), *size);
            assert_eq!(lock.as_ref(), blob_data.as_slice());
        }
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_generate_via_storage() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let secret_id = make_secret_id!("generated_blob");
        let spec = SecretSpec::new(Algorithm::None).extractable(true).size(64);

        let secret = storage.generate_secret(&spec, &secret_id).await?;
        assert_eq!(secret.metadata().algorithm, Algorithm::None);
        assert!(secret.metadata().is_blob());

        let blob = secret.as_blob()?;
        let data = blob.data().await?;
        let lock = data.lock().await?;
        assert_eq!(lock.len(), 64);

        let loaded = storage.load(&secret_id).await?;
        assert!(secret.eq_secret(&loaded).await?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_load_metadata() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto = create_crypto_factory(config.crypto_type).await?.new_crypto()?;
        let storage = create_test_storage(&config).await?;
        let secret =
            create_secret(&make_blob_test_data(32), "blob_meta", Algorithm::None, crypto).await?;
        let secret_id = secret.id().unwrap();

        storage.store(&secret, StoreMode::NewOnly).await?;

        let loaded_metadata = storage
            .load_metadata(secret_id)
            .await?
            .ok_or_else(|| VaultError::empty_metadata(""))?;
        assert_eq!(loaded_metadata.secret_id.as_ref().unwrap(), secret_id);
        assert_eq!(loaded_metadata.algorithm, Algorithm::None);
        assert!(loaded_metadata.is_blob());
        assert!(loaded_metadata.extractable);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_mixed_blob_and_keypair_storage() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;

        let blob = create_secret(
            &make_blob_test_data(64),
            "mixed_blob",
            Algorithm::None,
            crypto_factory.new_crypto()?,
        )
        .await?;
        storage.store(&blob, StoreMode::NewOnly).await?;

        let keypair = create_secret(
            make_ed25519_test_key_32().as_ref(),
            "mixed_keypair",
            Algorithm::Ed25519,
            crypto_factory.new_crypto()?,
        )
        .await?;
        storage.store(&keypair, StoreMode::NewOnly).await?;

        let metadata_list = storage.list_metadata().await?;
        assert_eq!(metadata_list.len(), 2);

        let blob_loaded = storage.load(blob.id().unwrap()).await?;
        assert!(blob_loaded.eq_secret(&blob).await?);
        assert!(blob_loaded.metadata().is_blob());
        assert!(blob_loaded.as_blob().is_ok());
        assert!(blob_loaded.as_keypair().is_err());

        let keypair_loaded = storage.load(keypair.id().unwrap()).await?;
        assert!(keypair_loaded.eq_secret(&keypair).await?);
        assert!(keypair_loaded.metadata().is_asymmetric());
        assert!(keypair_loaded.as_keypair().is_ok());
        assert!(keypair_loaded.as_blob().is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_concurrent_stores() -> anyhow::Result<()> {
    use std::sync::Arc;

    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        let mut set = tokio::task::JoinSet::new();

        for i in 0..10 {
            let storage = Arc::clone(&storage);
            let crypto = crypto_factory.new_crypto()?;
            set.spawn(async move {
                let blob_data = make_blob_test_data(32);
                let secret = create_secret(
                    &blob_data,
                    &format!("test_concurrent_blob_{}", i),
                    Algorithm::None,
                    crypto,
                )
                .await
                .unwrap();
                storage.store(&secret, StoreMode::CreateOrReplace).await.unwrap();

                secret.metadata().secret_id.clone()
            });
        }

        while let Some(result) = set.join_next().await {
            let secret_id = result?;
            assert!(secret_id.is_some());
        }

        let metadata_list = storage.list_metadata().await?;
        assert_eq!(metadata_list.len(), 10);

        for m in &metadata_list {
            assert_eq!(m.algorithm, Algorithm::None);
            assert!(m.is_blob());
        }
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_compare_data_across_storages() -> anyhow::Result<()> {
    let secret_id = make_secret_id!("blob_compare");
    let mut base_secret: Option<Secret> = None;
    let mut original_data: Option<Vec<u8>> = None;

    for config in fixture() {
        let storage = create_test_storage(&config).await?;

        let secret = if let Some(base_secret) = &base_secret {
            storage.store(base_secret, StoreMode::CreateOrReplace).await?;
            storage.load(&secret_id).await?
        } else {
            let spec = SecretSpec::new(Algorithm::None).extractable(true).size(64);
            let secret = storage.generate_secret(&spec, &secret_id).await?;

            let blob = secret.as_blob()?;
            let data = blob.data().await?;
            let lock = data.lock().await?;
            original_data = Some(lock.to_vec());

            base_secret = Some(secret);
            storage.load(&secret_id).await?
        };

        let blob = secret.as_blob()?;
        let base_blob = base_secret.as_ref().unwrap().as_blob()?;
        assert!(blob.eq_blob(base_blob).await?);

        assert_eq!(secret.metadata().algorithm, Algorithm::None);
        assert_eq!(secret.metadata().secret_id.as_ref().unwrap(), &secret_id);

        let data = blob.data().await?;
        let lock = data.lock().await?;
        assert_eq!(lock.len(), 64);
        assert_eq!(lock.as_ref(), original_data.as_ref().unwrap().as_slice());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_hierarchical_paths() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;
        let paths = vec![
            "blobs.group1.data_1",
            "blobs.group1.data_2",
            "blobs.group2.data_3",
            "blobs.root_data",
        ];

        for path in &paths {
            let secret = create_secret(
                &make_blob_test_data(32),
                path,
                Algorithm::None,
                crypto_factory.new_crypto()?,
            )
            .await?;
            storage.store(&secret, StoreMode::NewOnly).await?;
        }

        for path in &paths {
            let secret_id = (*path).into();
            let loaded = storage.load(&secret_id).await?;
            assert!(loaded.metadata().is_blob());
            assert_eq!(loaded.metadata().secret_id.as_ref().unwrap().as_str(), *path);
        }

        let metadata_list = storage.list_metadata().await?;
        assert_eq!(metadata_list.len(), 4);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_not_extractable_keypair_private_key_denied() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let secret_id = make_secret_id!("not_extractable");
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(false);
        let secret = storage.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        assert!(!keypair.extractable().await?);
        assert!(keypair.private_key().await.is_err());

        // Verify the property persists after load
        let loaded = storage.load(&secret_id).await?;
        let loaded_keypair = loaded.as_keypair()?;
        assert!(!loaded_keypair.extractable().await?);
        assert!(loaded_keypair.private_key().await.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_not_extractable_keypair_sign_verify() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let secret_id = make_secret_id!("not_extractable_sign");
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(false);
        storage.generate_secret(&spec, &secret_id).await?;

        let loaded = storage.load(&secret_id).await?;
        let keypair = loaded.as_keypair()?;
        assert!(!keypair.extractable().await?);

        let message = b"sign verify with non-extractable key";
        let signature = keypair.sign(message).await?;
        keypair.verify(message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_and_verify_basic() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let secret_id = make_secret_id!("sign_verify");
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = storage.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"storage sign verify test";

        let signature = keypair.sign(message).await?;
        assert_eq!(signature.len(), 64);

        keypair.verify(message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_verify_after_store_load() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let secret_id = make_secret_id!("sign_roundtrip");
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = storage.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"sign then reload";
        let signature = keypair.sign(message).await?;

        let loaded = storage.load(&secret_id).await?;
        let loaded_keypair = loaded.as_keypair()?;

        // Loaded key produces the same signature (Ed25519 is deterministic)
        let loaded_signature = loaded_keypair.sign(message).await?;
        assert_eq!(signature, loaded_signature);

        // Cross-verify
        loaded_keypair.verify(message, &signature).await?;
        keypair.verify(message, &loaded_signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_wrong_message_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let secret_id = make_secret_id!("verify_wrong_msg");
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = storage.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let signature = keypair.sign(b"correct").await?;

        let result = keypair.verify(b"incorrect", &signature).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_tampered_signature_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let secret_id = make_secret_id!("verify_tampered");
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = storage.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"tamper test";
        let mut signature = keypair.sign(message).await?;
        signature[0] ^= 0x01;

        let result = keypair.verify(message, &signature).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_wrong_key_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let storage = create_test_storage(&config).await?;
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);

        let id_a = make_secret_id!("key_a");
        let secret_a = storage.generate_secret(&spec, &id_a).await?;

        let id_b = make_secret_id!("key_b");
        let secret_b = storage.generate_secret(&spec, &id_b).await?;

        let kp_a = secret_a.as_keypair()?;
        let kp_b = secret_b.as_keypair()?;

        let message = b"cross-key test";
        let signature = kp_a.sign(message).await?;

        // Wrong key should fail
        let result = kp_b.verify(message, &signature).await;
        assert!(result.is_err());

        // Correct key should succeed
        kp_a.verify(message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_from_raw_key_data() -> anyhow::Result<()> {
    for config in fixture() {
        let crypto_factory = create_crypto_factory(config.crypto_type).await?;
        let storage = create_test_storage(&config).await?;

        let key_data = make_ed25519_test_key_32();
        let secret =
            create_secret(&key_data, "raw_sign", Algorithm::Ed25519, crypto_factory.new_crypto()?)
                .await?;

        storage.store(&secret, StoreMode::NewOnly).await?;

        let loaded = storage.load(secret.id().unwrap()).await?;
        let keypair = loaded.as_keypair()?;

        let message = b"raw key sign test";
        let signature = keypair.sign(message).await?;
        assert_eq!(signature.len(), 64);
        keypair.verify(message, &signature).await?;

        // Original and loaded should produce the same signature
        let original_keypair = secret.as_keypair()?;
        let original_signature = original_keypair.sign(message).await?;
        assert_eq!(signature, original_signature);
    }

    Ok(())
}
