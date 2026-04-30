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
        blob_in_memory::BlobInMemory, crypto_trait::Crypto, key_material::KeyMaterial,
        key_pair_in_memory::KeyPairInMemory, symmetric_key_in_memory::SymmetricKeyInMemory,
    },
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    types::{
        algorithm::Algorithm, metadata::Metadata, payload::PayloadType, secret::Secret,
        secret_id::SecretId, secret_spec::SecretSpec, store_mode::StoreMode,
    },
};
use std::sync::Arc;

pub fn encrypt(
    master_key: &KeyMaterial,
    data: &ProtectedMemory,
    metadata: &Metadata,
    crypto: &dyn Crypto,
) -> anyhow::Result<Vec<u8>> {
    let encoded_metadata = serde_json::to_vec(metadata)
        .map_err(|e| VaultError::internal("Metadata serialization failed").with_source(e))?;

    // Format: [data_len: u32][meta_len: u32][data][meta]
    let mut all_data = ProtectedMemoryInner::new(0)?;
    {
        let mut handle = all_data.write_handle()?;

        let data_len = data.len();
        let meta_len = encoded_metadata.len();

        handle.extend_from_slice(&(data_len as u32).to_le_bytes())?;
        handle.extend_from_slice(&(meta_len as u32).to_le_bytes())?;

        if data_len > 0 {
            let read_lock_data = data.lock()?;
            handle.extend_from_slice(&read_lock_data)?;
        }

        if meta_len > 0 {
            handle.extend_from_slice(&encoded_metadata)?;
        }
    }

    let all_data: ProtectedMemory = all_data.into();
    let read_lock_all_data = all_data.lock()?;

    crypto.encrypt(master_key, &read_lock_all_data, Algorithm::Aes256Gcm)
}

pub fn decrypt(
    master_key: &KeyMaterial,
    ciphertext: &[u8],
    crypto: &dyn Crypto,
) -> anyhow::Result<(ProtectedMemory, Metadata)> {
    // Format: [data_len: u32][meta_len: u32][data][meta]
    let decrypted_data = crypto.decrypt(master_key, ciphertext, Algorithm::Aes256Gcm)?;
    let decrypted_data_len = decrypted_data.len();

    if decrypted_data_len < (2 * size_of::<u32>()) {
        anyhow::bail!(VaultError::internal(format!("wrong data size: {}", decrypted_data_len)))
    }

    let read_lock_decrypted_data = decrypted_data.lock()?;
    let read_data = &read_lock_decrypted_data as &[u8];

    let (data_len_bytes, rest) = read_data.split_at(size_of::<u32>());
    let (meta_len_bytes, rest) = rest.split_at(size_of::<u32>());

    let data_len = u32::from_le_bytes(data_len_bytes.try_into().unwrap()) as usize;
    let meta_len = u32::from_le_bytes(meta_len_bytes.try_into().unwrap()) as usize;
    let expected_len = 2 * size_of::<u32>() + data_len + meta_len;

    if decrypted_data_len != expected_len {
        anyhow::bail!(VaultError::internal(format!(
            "wrong data size: {}, expected to be {}",
            decrypted_data_len, expected_len
        )))
    }

    let (data_bytes, rest) = rest.split_at(data_len);
    let res_data: ProtectedMemory = ProtectedMemoryInner::from_slice(data_bytes)?.into();
    let (meta_bytes, _) = rest.split_at(meta_len);
    let meta = serde_json::from_slice::<Metadata>(meta_bytes)
        .map_err(|e| VaultError::internal("Metadata deserialization failed").with_source(e))?;

    Ok((res_data, meta))
}

pub fn prepare_to_store(
    data: &ProtectedMemory,
    metadata: &Metadata,
    mode: StoreMode,
    exists: bool,
    key_material: &KeyMaterial,
    crypto: &dyn Crypto,
) -> anyhow::Result<Vec<u8>> {
    let secret_id = metadata
        .secret_id
        .as_ref()
        .ok_or_else(|| VaultError::empty_secret_id("Failed to prepare secret for storage"))?;

    match mode {
        StoreMode::NewOnly => {
            if exists {
                anyhow::bail!(VaultError::already_exists(format!(
                    "Secret with id '{}' already exists",
                    secret_id
                )))
            }
        }
        StoreMode::ReplaceExists => {
            if !exists {
                anyhow::bail!(VaultError::not_found(format!("Secret '{}' not found", secret_id)))
            }
        }
        StoreMode::CreateOrReplace => {}
    }
    encrypt(key_material, data, metadata, crypto)
}

pub fn generate_secret_in_memory(
    spec: &SecretSpec,
    secret_id: &SecretId,
    crypto: Arc<dyn Crypto>,
) -> anyhow::Result<Secret> {
    let key_material = KeyMaterial::generate_new(spec.algorithm, spec.size, crypto.as_ref())?;
    let metadata = Metadata::from_spec(Some(secret_id), spec);
    let secret = match spec.algorithm.payload_type() {
        PayloadType::SymmetricKey => Secret::SymmetricKey {
            key: Box::new(SymmetricKeyInMemory::new(&metadata, key_material)),
        },
        PayloadType::KeyPair => Secret::KeyPair {
            keypair: Box::new(KeyPairInMemory::new(&metadata, key_material, crypto)),
        },
        PayloadType::Blob => {
            let secret_key = key_material
                .secret_key
                .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

            Secret::Blob { blob: Box::new(BlobInMemory::new(&metadata, secret_key)) }
        }
    };

    Ok(secret)
}

pub mod hex_string {
    use serde::Deserialize;

    pub fn serialize<S>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<u8>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        hex::decode(&s).map_err(serde::de::Error::custom)
    }
}

pub mod b64 {
    use serde::{de::Error as _, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(bytes: &[u8], ser: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        ser.serialize_str(&base64::encode(bytes))
    }

    pub fn deserialize<'de, D>(de: D) -> Result<Vec<u8>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(de)?;
        base64::decode(s.as_bytes()).map_err(D::Error::custom)
    }
}
