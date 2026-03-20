/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::key_material::KeyMaterial, errors::error::VaultError,
    memory::protected_memory::ProtectedMemory,
};

#[tokio::test]
async fn test_symmetric_key_deserialize() -> anyhow::Result<()> {
    let key_data = b"this_is_a_32_byte_symmetric_key!";
    let private_key = ProtectedMemory::from_slice(key_data).await?;
    let key_material = KeyMaterial::new(Some(private_key), None).await?;

    let plaintext = key_material.serialize().await?;
    let plaintext_lock = plaintext.lock().await?;
    //[secret_key_len: u32][public_key_len: u32][secret_key][public_key]
    assert!(plaintext_lock.len() == (size_of::<u32>() + size_of::<u32>() + key_data.len()));

    let key_material_2 = KeyMaterial::deserialize(&plaintext_lock).await?;
    assert!(key_material.public_key.is_none());
    assert!(key_material_2.public_key.is_none());

    {
        let lock1 = key_material
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key(""))?
            .lock()
            .await?;
        let lock2 = key_material_2
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key(""))?
            .lock()
            .await?;

        assert_eq!(&*lock1, &*lock2);
    }

    Ok(())
}

#[tokio::test]
async fn test_asymmetric_key_deserialize() -> anyhow::Result<()> {
    let private_key_data = b"private_key";
    let public_key_data = b"public_key";
    let private_key = ProtectedMemory::from_slice(private_key_data).await?;
    let key_material = KeyMaterial::new(Some(private_key), Some(public_key_data.to_vec())).await?;

    let plaintext = key_material.serialize().await?;
    let plaintext_lock = plaintext.lock().await?;
    //[secret_key_len: u32][public_key_len: u32][secret_key][public_key]
    assert!(
        plaintext_lock.len()
            == (size_of::<u32>()
                + size_of::<u32>()
                + private_key_data.len()
                + public_key_data.len())
    );

    let key_material_2 = KeyMaterial::deserialize(&plaintext_lock).await?;
    assert_eq!(
        key_material.public_key.as_ref().unwrap(),
        key_material_2.public_key.as_ref().unwrap()
    );

    {
        let lock1 = key_material
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key(""))?
            .lock()
            .await?;
        let lock2 = key_material_2
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key(""))?
            .lock()
            .await?;

        assert_eq!(&*lock1, &*lock2);
    }

    Ok(())
}

#[tokio::test]
async fn test_deserialize_overflow() {
    let mut invalid_data = vec![0u8; 8];

    invalid_data[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
    invalid_data[4..8].copy_from_slice(&u32::MAX.to_le_bytes());

    let result = KeyMaterial::deserialize(&invalid_data).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_deserialize_extra_data() {
    let private_key_data = b"test";
    let mut plaintext = vec![0u8; 8 + private_key_data.len() + 10]; // Extra 10 bytes

    plaintext[0..4].copy_from_slice(&(private_key_data.len() as u32).to_le_bytes());
    plaintext[4..8].copy_from_slice(&0u32.to_le_bytes());
    plaintext[8..8 + private_key_data.len()].copy_from_slice(private_key_data);

    let result = KeyMaterial::deserialize(&plaintext).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_large_symmetric_key() -> anyhow::Result<()> {
    let large_key_data = vec![42u8; 10 * 1024];
    let private_key = ProtectedMemory::from_slice(&large_key_data).await?;
    let key_material = KeyMaterial::new(Some(private_key), None).await?;
    let plaintext = key_material.serialize().await?;
    let plaintext_lock = plaintext.lock().await?;
    let restored = KeyMaterial::deserialize(&plaintext_lock).await?;
    let restored_private = restored
        .secret_key
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_key(""))?
        .lock()
        .await?;

    assert_eq!(restored_private.len(), large_key_data.len());
    assert_eq!(&*restored_private, &large_key_data[..]);

    Ok(())
}

#[tokio::test]
async fn test_large_asymmetric_key() -> anyhow::Result<()> {
    let large_private = vec![1u8; 5 * 1024];
    let large_public = vec![2u8; 2 * 1024];
    let private_key = ProtectedMemory::from_slice(&large_private).await?;
    let key_material = KeyMaterial::new(Some(private_key), Some(large_public.clone())).await?;
    let plaintext = key_material.serialize().await?;
    let plaintext_lock = plaintext.lock().await?;
    let restored = KeyMaterial::deserialize(&plaintext_lock).await?;
    let restored_private = restored
        .secret_key
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_key(""))?
        .lock()
        .await?;

    assert_eq!(&*restored_private, &large_private[..]);
    assert_eq!(restored.public_key.as_ref().unwrap(), &large_public);

    Ok(())
}

#[tokio::test]
async fn test_symmetric_with_empty_key() -> anyhow::Result<()> {
    let key_data = vec![];
    let key = ProtectedMemory::from_slice(&key_data).await?;
    let result = KeyMaterial::new(Some(key), None).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_asymmetric_with_empty_public_key() -> anyhow::Result<()> {
    let private_key_data = b"private_key";
    let private_key = ProtectedMemory::from_slice(private_key_data).await?;
    KeyMaterial::new(Some(private_key), None).await?;

    Ok(())
}

#[tokio::test]
async fn test_asymmetric_with_empty_private_key() -> anyhow::Result<()> {
    let private_key_data = vec![];
    let private_key = ProtectedMemory::from_slice(&private_key_data).await?;
    let public_key = b"public_key";
    let result = KeyMaterial::new(Some(private_key), Some(public_key.to_vec())).await;
    assert!(result.is_err());

    Ok(())
}

#[tokio::test]
async fn test_multiple_roundtrips() -> anyhow::Result<()> {
    let private_key_data = b"original_private_key";
    let public_key_data = b"original_public_key";
    let private_key = ProtectedMemory::from_slice(private_key_data).await?;
    let mut current = KeyMaterial::new(Some(private_key), Some(public_key_data.to_vec())).await?;

    for _ in 0..5 {
        let plaintext = current.serialize().await?;
        let plaintext_lock = plaintext.lock().await?;
        current = KeyMaterial::deserialize(&plaintext_lock).await?;
    }

    let final_private =
        current.secret_key.as_ref().ok_or_else(|| VaultError::empty_secret_key(""))?.lock().await?;
    assert_eq!(&*final_private, private_key_data);
    assert_eq!(current.public_key.as_ref().unwrap(), public_key_data);

    Ok(())
}

#[tokio::test]
async fn test_concurrent_serialization() -> anyhow::Result<()> {
    use std::sync::Arc;

    let private_key_data = b"concurrent_test_key_data_here";
    let private_key = ProtectedMemory::from_slice(private_key_data).await?;
    let key_material = Arc::new(KeyMaterial::new_symmetric_key(private_key).await?);

    let mut handles = vec![];

    for _ in 0..10 {
        let km = key_material.clone();
        let handle = tokio::spawn(async move { km.serialize().await });
        handles.push(handle);
    }

    for handle in handles {
        let plaintext = handle.await??;
        let plaintext_lock = plaintext.lock().await?;

        let restored = KeyMaterial::deserialize(&plaintext_lock).await?;
        let restored_private = restored
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?
            .lock()
            .await?;

        assert_eq!(&*restored_private, private_key_data);
    }

    Ok(())
}
