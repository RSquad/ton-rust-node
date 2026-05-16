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
        crypto_trait::Crypto,
        factory::{CryptoFactory, DefaultCryptoFactory},
    },
    errors::error::VaultError,
    make_secret_id,
    storage::storage_trait::ListMode,
    tests::fixture::*,
    types::{
        algorithm::Algorithm,
        secret::{Secret, SecretDataType},
        secret_spec::SecretSpec,
        store_mode::StoreMode,
    },
};
use rand::RngCore;
use std::{future::Future, sync::Arc};

#[tokio::test]
#[serial_test::serial]
async fn test_generate_ed25519_key() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "test_symmetric_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        assert_eq!(secret.id().unwrap(), &secret_id);
        assert_eq!(secret.metadata().algorithm, Algorithm::Ed25519);
        assert!(secret.metadata().is_asymmetric());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_generate_blob() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "test_blob".into();
        let spec = SecretSpec::new(Algorithm::None).extractable(true).size(64);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        assert_eq!(secret.id().unwrap(), &secret_id);
        assert_eq!(secret.metadata().algorithm, Algorithm::None);
        assert!(secret.metadata().is_blob());
        assert!(secret.metadata().extractable);

        let blob = secret.as_blob()?;
        let data = blob.data();
        let lock = data.lock()?;
        assert_eq!(lock.len(), 64);

        let loaded = vault.load(&secret_id).await?;
        assert!(secret.eq_secret(&loaded)?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_generate_not_extractable_key() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "not_extractable_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(false);

        vault.generate_secret(&spec, &secret_id).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_generate_extractable_key() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "extractable_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);

        vault.generate_secret(&spec, &secret_id).await?;

        let secret = vault.load(&secret_id).await?;
        assert_eq!(secret.id().unwrap(), &secret_id);
        assert_eq!(secret.metadata().algorithm, Algorithm::Ed25519);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_get_nonexistent_key_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "nonexistent".into();
        let result = vault.load(&secret_id).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_put_secret_new_only() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "put_test_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;
        let result = vault.store(&secret, StoreMode::NewOnly).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_put_secret_replace_exists() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        // Store
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            SecretDataType::Ed25519PvtKey,
            "create_or_replace_key",
            Algorithm::Ed25519,
            crypto.clone(),
        )
        .await?;

        vault.store(&secret, StoreMode::NewOnly).await?;
        vault.store(&secret, StoreMode::ReplaceExists).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_put_secret_create_or_replace() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        // Store
        let secret = create_secret(
            make_ed25519_test_key_32().as_ref(),
            SecretDataType::Ed25519PvtKey,
            "create_or_replace_key",
            Algorithm::Ed25519,
            crypto.clone(),
        )
        .await?;

        vault.store(&secret, StoreMode::NewOnly).await?;
        vault.store(&secret, StoreMode::CreateOrReplace).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_delete_existing_key() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "delete_test_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        vault.generate_secret(&spec, &secret_id).await?;
        vault.delete(&secret_id).await?;
        assert!(vault.load(&secret_id).await.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_delete_nonexistent_key_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "nonexistent".into();
        let result = vault.delete(&secret_id).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_exists_true() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "exists_test_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        vault.generate_secret(&spec, &secret_id).await?;
        assert!(vault.load(&secret_id).await.is_ok());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_exists_false() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "nonexistent".into();
        assert!(vault.load(&secret_id).await.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_load_metadata() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "metadata_test_key".into();
        let mut spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        spec.tags.insert("test".to_string(), "value".to_string());
        let secret = vault.generate_secret(&spec, &secret_id).await?;
        let metadata = vault.load_metadata(&secret_id).await?;
        assert!(secret.metadata().eq(&metadata));
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_list_metadata_empty() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let metadata_list = vault.list_metadata(ListMode::OnlyNeeded).await?;
        assert!(metadata_list.is_empty());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_list_metadata_multiple_keys() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);

        for i in 0..5 {
            let secret_id = format!("key_{}", i).into();
            vault.generate_secret(&spec, &secret_id).await?;
        }

        let metadata_list = vault.list_metadata(ListMode::All).await?;
        assert_eq!(metadata_list.len(), 5);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_hierarchical_key_paths() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);

        let paths = vec![
            "app.production.db.key",
            "app.production.api.key",
            "app.staging.db.key",
            "app.staging.api.key",
        ];

        for path in &paths {
            let secret_id = (*path).into();
            vault.generate_secret(&spec, &secret_id).await?;
        }

        for path in &paths {
            let secret_id = (*path).into();
            assert!(vault.load(&secret_id).await.is_ok());
        }
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_concurrent_key_generation() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = Arc::new(create_test_vault(&config).await?);
        let mut handles = vec![];

        for i in 0..10 {
            let vault_clone = vault.clone();
            let handle = tokio::spawn(async move {
                let secret_id = format!("concurrent_key_{}", i).into();
                let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
                vault_clone.generate_secret(&spec, &secret_id).await
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await??;
        }

        let metadata_list = vault.list_metadata(ListMode::All).await?;
        assert_eq!(metadata_list.len(), 10);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_concurrent_read_operations() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = Arc::new(create_test_vault(&config).await?);
        let secret_id = "shared_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);

        vault.generate_secret(&spec, &secret_id).await?;

        let mut handles = vec![];

        for _ in 0..10 {
            let vault_clone = vault.clone();
            let id = secret_id.clone();
            let handle = tokio::spawn(async move { vault_clone.load(&id).await });
            handles.push(handle);
        }

        for handle in handles {
            handle.await??;
        }
    }

    Ok(())
}

async fn test_secret_lifecycle<F, Fut>(gen_secret: F) -> anyhow::Result<()>
where
    F: Fn(String, Arc<dyn Crypto>) -> Fut,
    Fut: Future<Output = anyhow::Result<Secret>> + Send,
{
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        let metadata = vault.list_metadata(ListMode::All).await?;
        assert_eq!(metadata.len(), 0);

        for id in 0..10 {
            let secret_id = make_secret_id!("lifecycle_key", id);

            // Store
            let secret = gen_secret(secret_id.to_string(), crypto.clone()).await?;
            vault.store(&secret, StoreMode::NewOnly).await?;

            let metadata = vault.list_metadata(ListMode::All).await?;
            assert_eq!(metadata.len(), id + 1);

            // Get
            let retrieved = vault.load(&secret_id).await?;
            assert!(secret.eq_secret(&retrieved)?);

            assert_eq!(metadata.len(), id + 1);

            // Update
            vault.store(&secret, StoreMode::CreateOrReplace).await?;
            let res = vault.load(&secret_id).await;
            assert!(res.is_ok());

            assert_eq!(metadata.len(), id + 1);
        }

        let metadata = vault.list_metadata(ListMode::All).await?;
        assert_eq!(metadata.len(), 10);

        for id in 0..10 {
            let secret_id = make_secret_id!("lifecycle_key", id);

            // Delete
            vault.delete(&secret_id).await?;
            let res = vault.load(&secret_id).await;
            assert!(res.is_err());

            let metadata = vault.list_metadata(ListMode::All).await?;
            assert_eq!(metadata.len(), 9 - id);
        }

        let metadata = vault.list_metadata(ListMode::All).await?;
        assert_eq!(metadata.len(), 0);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_lifecycle_ed25519_blob() -> anyhow::Result<()> {
    // ed25519
    {
        let create_secret_fn = |secret_id: String, crypto: Arc<dyn Crypto>| async move {
            create_secret(
                make_ed25519_test_key_32().as_ref(),
                SecretDataType::Ed25519PvtKey,
                &secret_id,
                Algorithm::Ed25519,
                crypto,
            )
            .await
        };

        test_secret_lifecycle(create_secret_fn).await?;
    }

    // blob
    {
        let create_secret_fn = |secret_id: String, crypto: Arc<dyn Crypto>| async move {
            let mut blob_data = [0u8; 48];
            rand::thread_rng().fill_bytes(&mut blob_data);

            create_secret(
                &blob_data,
                SecretDataType::Raw,
                secret_id.as_str(),
                Algorithm::None,
                crypto,
            )
            .await
        };

        test_secret_lifecycle(create_secret_fn).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_generate_different_sizes() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        for (i, size) in [1usize, 16, 32, 64, 128, 256, 1024].iter().enumerate() {
            let secret_id = format!("blob_size_{}", i).into();
            let spec = SecretSpec::new(Algorithm::None).extractable(true).size(*size);
            let secret = vault.generate_secret(&spec, &secret_id).await?;

            let blob = secret.as_blob()?;
            let data = blob.data();
            let lock = data.lock()?;
            assert_eq!(lock.len(), *size);

            let loaded = vault.load(&secret_id).await?;
            assert!(secret.eq_secret(&loaded)?);
        }
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_put_and_get() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        let mut blob_data = [0u8; 64];
        rand::thread_rng().fill_bytes(&mut blob_data);
        let secret = create_secret(
            &blob_data,
            SecretDataType::Raw,
            "put_blob",
            Algorithm::None,
            crypto.clone(),
        )
        .await?;

        vault.store(&secret, StoreMode::NewOnly).await?;

        let loaded = vault.load(&"put_blob".into()).await?;
        assert!(loaded.eq_secret(&secret)?);
        assert!(loaded.metadata().is_blob());

        let loaded_blob = loaded.as_blob()?;
        let loaded_data = loaded_blob.data();
        let lock = loaded_data.lock()?;
        assert_eq!(lock.as_ref(), blob_data.as_slice());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_delete() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "delete_blob".into();
        let spec = SecretSpec::new(Algorithm::None).extractable(true).size(32);
        vault.generate_secret(&spec, &secret_id).await?;

        vault.delete(&secret_id).await?;
        assert!(vault.load(&secret_id).await.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_metadata_tags() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "tagged_blob".into();
        let mut spec = SecretSpec::new(Algorithm::None).extractable(true).size(32);
        spec.tags.insert("purpose".to_string(), "config-data".to_string());
        spec.tags.insert("env".to_string(), "test".to_string());

        let secret = vault.generate_secret(&spec, &secret_id).await?;
        let metadata = vault.load_metadata(&secret_id).await?;
        assert!(secret.metadata().eq(&metadata));
        assert_eq!(metadata.get_tag("purpose"), Some("config-data"));
        assert_eq!(metadata.get_tag("env"), Some("test"));
        assert!(metadata.is_blob());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_replace() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        let mut data1 = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut data1);
        let secret1 = create_secret(
            &data1,
            SecretDataType::Raw,
            "replace_blob",
            Algorithm::None,
            crypto.clone(),
        )
        .await?;
        vault.store(&secret1, StoreMode::NewOnly).await?;

        let mut data2 = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut data2);
        let secret2 = create_secret(
            &data2,
            SecretDataType::Raw,
            "replace_blob",
            Algorithm::None,
            crypto.clone(),
        )
        .await?;
        vault.store(&secret2, StoreMode::CreateOrReplace).await?;

        let loaded = vault.load(&"replace_blob".into()).await?;
        assert!(loaded.eq_secret(&secret2)?);
        assert!(!loaded.eq_secret(&secret1)?);

        let loaded_blob = loaded.as_blob()?;
        let loaded_data = loaded_blob.data();
        let lock = loaded_data.lock()?;
        assert_eq!(lock.as_ref(), data2.as_slice());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_eq_blob_same_data() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;
    let blob_data = [42u8; 32];

    let secret1 = create_secret(
        &blob_data,
        SecretDataType::Raw,
        "eq_blob_1",
        Algorithm::None,
        crypto.clone(),
    )
    .await?;
    let secret2 = create_secret(
        &blob_data,
        SecretDataType::Raw,
        "eq_blob_1",
        Algorithm::None,
        crypto.clone(),
    )
    .await?;

    let blob1 = secret1.as_blob()?;
    let blob2 = secret2.as_blob()?;
    assert!(blob1.eq_blob(blob2)?);

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_eq_blob_different_data() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    let secret1 =
        create_secret(&[1u8; 32], SecretDataType::Raw, "neq_blob", Algorithm::None, crypto.clone())
            .await?;
    let secret2 =
        create_secret(&[2u8; 32], SecretDataType::Raw, "neq_blob", Algorithm::None, crypto.clone())
            .await?;

    let blob1 = secret1.as_blob()?;
    let blob2 = secret2.as_blob()?;
    assert!(!blob1.eq_blob(blob2)?);

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_as_wrong_type_fails() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;

    let blob_secret =
        create_secret(&[0u8; 32], SecretDataType::Raw, "type_blob", Algorithm::None, crypto)
            .await?;
    assert!(blob_secret.as_blob().is_ok());
    assert!(blob_secret.as_keypair().is_err());
    assert!(blob_secret.as_symmetric_key().is_err());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_blob_mixed_with_keypairs_in_vault() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        let blob_id = "mixed_blob".into();
        let blob_spec = SecretSpec::new(Algorithm::None).extractable(true).size(64);
        vault.generate_secret(&blob_spec, &blob_id).await?;

        let key_id = "mixed_key".into();
        let key_spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        vault.generate_secret(&key_spec, &key_id).await?;

        let metadata_list = vault.list_metadata(ListMode::All).await?;
        assert_eq!(metadata_list.len(), 2);

        let blob_loaded = vault.load(&blob_id).await?;
        assert!(blob_loaded.metadata().is_blob());
        assert!(blob_loaded.as_blob().is_ok());

        let key_loaded = vault.load(&key_id).await?;
        assert!(key_loaded.metadata().is_asymmetric());
        assert!(key_loaded.as_keypair().is_ok());

        assert!(!blob_loaded.eq_secret(&key_loaded)?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_not_extractable_keypair_private_key_denied() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "not_extractable_kp".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(false);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;

        // extractable() should report false
        assert!(!keypair.extractable());

        // public_key() should still work
        let pub_key = keypair.public_key();
        assert!(pub_key.is_some());
        assert_eq!(pub_key.unwrap().len(), 32);

        // private_key() must fail
        let result = keypair.private_key();
        assert!(result.is_err());

        if let Some(e) = result.unwrap_err().downcast_ref::<VaultError>() {
            assert_eq!(e.code(), VaultError::NOT_EXTRACTABLE);
        } else {
            anyhow::bail!("Wrong value");
        }
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_not_extractable_keypair_sign_verify_still_works() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "not_extractable_sign".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(false);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"test message for signing";

        // sign and verify should work even when non-extractable
        let signature = keypair.sign(message).await?;
        keypair.verify(message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_not_extractable_keypair_roundtrip_via_storage() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "not_extractable_rt".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(false);
        vault.generate_secret(&spec, &secret_id).await?;

        // Load from storage and verify non-extractable property persists
        let loaded = vault.load(&secret_id).await?;
        let keypair = loaded.as_keypair()?;

        assert!(!keypair.extractable());
        assert!(keypair.private_key().is_err());

        // sign/verify still works after round-trip
        let message = b"roundtrip test";
        let signature = keypair.sign(message).await?;
        keypair.verify(message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_not_extractable_keypair_from_raw_data() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;
    let secret = create_secret_extractable(
        make_ed25519_test_key_32().as_ref(),
        SecretDataType::Ed25519PvtKey,
        "not_extractable_raw",
        Algorithm::Ed25519,
        false,
        crypto,
    )
    .await?;

    let keypair = secret.as_keypair()?;
    assert!(!keypair.extractable());
    assert!(keypair.private_key().is_err());

    // public key should still be available
    assert!(keypair.public_key().is_some());

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_not_extractable_vs_extractable_eq_keypair() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;

        let ext_id = "extractable_eq".into();
        let ext_spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let ext_secret = vault.generate_secret(&ext_spec, &ext_id).await?;

        let non_ext_id = "not_extractable_eq".into();
        let non_ext_spec = SecretSpec::new(Algorithm::Ed25519).extractable(false);
        let non_ext_secret = vault.generate_secret(&non_ext_spec, &non_ext_id).await?;

        // Different extractable flags means not equal
        assert!(!ext_secret.eq_secret(&non_ext_secret)?);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_and_verify() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "sign_verify_key".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"hello world";

        let signature = keypair.sign(message).await?;
        assert!(!signature.is_empty());

        // Ed25519 signatures are 64 bytes
        assert_eq!(signature.len(), 64);

        keypair.verify(message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_deterministic() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "deterministic_sign".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"deterministic test message";

        let sig1 = keypair.sign(message).await?;
        let sig2 = keypair.sign(message).await?;

        // Ed25519 signing is deterministic
        assert_eq!(sig1, sig2);
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_wrong_message_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "verify_wrong_msg".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let signature = keypair.sign(b"correct message").await?;

        let result = keypair.verify(b"wrong message", &signature).await;
        assert!(result.is_err());
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_tampered_signature_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "verify_tampered".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"tamper test";
        let mut signature = keypair.sign(message).await?;

        // Flip a bit in the signature
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
        let vault = create_test_vault(&config).await?;
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);

        let id_a = "sign_key_a".into();
        let secret_a = vault.generate_secret(&spec, &id_a).await?;

        let id_b = "sign_key_b".into();
        let secret_b = vault.generate_secret(&spec, &id_b).await?;

        let keypair_a = secret_a.as_keypair()?;
        let keypair_b = secret_b.as_keypair()?;

        let message = b"cross-key test";
        let signature = keypair_a.sign(message).await?;

        // Verify with the wrong key should fail
        let result = keypair_b.verify(message, &signature).await;
        assert!(result.is_err());

        // Verify with the correct key should succeed
        keypair_a.verify(message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_empty_message() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "sign_empty".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let signature = keypair.sign(b"").await?;
        assert_eq!(signature.len(), 64);

        keypair.verify(b"", &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_large_message() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "sign_large".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let large_message = vec![0xABu8; 1024 * 64]; // 64 KB

        let signature = keypair.sign(&large_message).await?;
        keypair.verify(&large_message, &signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_verify_after_storage_roundtrip() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "sign_roundtrip".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"roundtrip sign test";
        let signature = keypair.sign(message).await?;

        // Load from storage
        let loaded = vault.load(&secret_id).await?;
        let loaded_keypair = loaded.as_keypair()?;

        // Loaded key should produce the same signature
        let loaded_signature = loaded_keypair.sign(message).await?;
        assert_eq!(signature, loaded_signature);

        // Loaded key should verify original signature
        loaded_keypair.verify(message, &signature).await?;

        // Original key should verify loaded key's signature
        keypair.verify(message, &loaded_signature).await?;
    }

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_verify_from_raw_key() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;
    let key_data = make_ed25519_test_key_32();
    let secret = create_secret(
        &key_data,
        SecretDataType::Ed25519PvtKey,
        "raw_sign_key",
        Algorithm::Ed25519,
        crypto,
    )
    .await?;

    let keypair = secret.as_keypair()?;
    let message = b"raw key sign test";

    let signature = keypair.sign(message).await?;
    assert_eq!(signature.len(), 64);

    keypair.verify(message, &signature).await?;

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_verify_from_expanded_key() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;
    let pvt_key_data = make_ed25519_test_key_32();
    let exp_key = crypto.exp_key_from_pvt(Algorithm::Ed25519, &pvt_key_data)?;
    let exp_key_bytes: Vec<u8> = exp_key.lock()?.to_vec();

    let secret = create_secret(
        &exp_key_bytes,
        SecretDataType::Ed25519ExpKey,
        "exp_sign_key",
        Algorithm::Ed25519,
        crypto,
    )
    .await?;

    let keypair = secret.as_keypair()?;
    let message = b"expanded key sign test";

    let signature = keypair.sign(message).await?;
    assert_eq!(signature.len(), 64);

    keypair.verify(message, &signature).await?;

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_sign_matches_pvt_vs_exp_key() -> anyhow::Result<()> {
    let crypto = DefaultCryptoFactory {}.new_crypto()?;
    let pvt_key_data = make_ed25519_test_key_32();
    let exp_key = crypto.exp_key_from_pvt(Algorithm::Ed25519, &pvt_key_data)?;
    let exp_key_bytes: Vec<u8> = exp_key.lock()?.to_vec();

    let secret_pvt = create_secret(
        &pvt_key_data,
        SecretDataType::Ed25519PvtKey,
        "pvt_key",
        Algorithm::Ed25519,
        crypto.clone(),
    )
    .await?;

    let secret_exp = create_secret(
        &exp_key_bytes,
        SecretDataType::Ed25519ExpKey,
        "exp_key",
        Algorithm::Ed25519,
        crypto.clone(),
    )
    .await?;

    let kp_pvt = secret_pvt.as_keypair()?;
    let kp_exp = secret_exp.as_keypair()?;

    // Same public key
    assert_eq!(kp_pvt.public_key(), kp_exp.public_key());

    // Same signature
    let message = b"cross key type test";
    let sig_pvt = kp_pvt.sign(message).await?;
    let sig_exp = kp_exp.sign(message).await?;
    assert_eq!(sig_pvt, sig_exp);

    // Cross-verify
    kp_pvt.verify(message, &sig_exp).await?;
    kp_exp.verify(message, &sig_pvt).await?;

    Ok(())
}

#[tokio::test]
#[serial_test::serial]
async fn test_verify_truncated_signature_fails() -> anyhow::Result<()> {
    for config in fixture() {
        let vault = create_test_vault(&config).await?;
        let secret_id = "verify_truncated".into();
        let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
        let secret = vault.generate_secret(&spec, &secret_id).await?;

        let keypair = secret.as_keypair()?;
        let message = b"truncation test";
        let signature = keypair.sign(message).await?;

        // Truncated signature should fail
        let result = keypair.verify(message, &signature[..32]).await;
        assert!(result.is_err());
    }

    Ok(())
}
