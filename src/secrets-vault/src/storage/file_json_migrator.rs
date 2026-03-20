/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::{crypto_trait::Crypto, key_material::KeyMaterial},
    errors::error::VaultError,
    memory::protected_memory::ProtectedMemory,
    storage::utils::{decrypt, encrypt},
    types::algorithm::Algorithm,
};

pub(crate) async fn migrate_tree_node_v1_to_v2(
    node: &mut serde_json::Value,
    master_key: &KeyMaterial,
    crypto: &dyn Crypto,
) -> anyhow::Result<()> {
    if let Some(encrypted_hex) = node
        .get("secret")
        .and_then(|s| s.get("encrypted_data"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
    {
        let encrypted_bytes = hex::decode(&encrypted_hex)?;
        if let Some(new_encrypted) =
            migrate_secret_v1_to_v2(&encrypted_bytes, master_key, crypto).await?
        {
            node["secret"]["encrypted_data"] =
                serde_json::Value::String(hex::encode(&new_encrypted));
        }
    }

    if let Some(children) = node.get_mut("children") {
        if let Some(children_map) = children.as_object_mut() {
            for (_key, child) in children_map.iter_mut() {
                Box::pin(migrate_tree_node_v1_to_v2(child, master_key, crypto)).await?;
            }
        }
    }

    Ok(())
}

async fn migrate_secret_v1_to_v2(
    encrypted_data: &[u8],
    master_key: &KeyMaterial,
    crypto: &dyn Crypto,
) -> anyhow::Result<Option<Vec<u8>>> {
    let (data, metadata) = decrypt(master_key, encrypted_data, crypto).await?;

    if metadata.algorithm != Algorithm::Ed25519 {
        return Ok(None);
    }

    let new_key_bytes = {
        let key_material = KeyMaterial::deserialize(&data.lock().await?).await?;

        let secret_key = key_material
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

        // Fix Ed25519 key generation that incorrectly stored 64-byte expanded keys instead of 32-byte seeds,
        // causing pub_key_from_pvt_ed25519 to return wrong public keys for extractable keys.
        if secret_key.len().await == 32 {
            return Ok(None);
        }

        // Keep first 32 bytes of Ed25519 private key
        let secret_key_lock = secret_key.lock().await?;
        let secret_key_data: &[u8] = &secret_key_lock;
        let secret_key: &[u8; 32] = secret_key_data
            .get(..32)
            .ok_or_else(|| {
                anyhow::anyhow!("secret key too short: {} bytes", secret_key_data.len())
            })?
            .try_into()?;

        let new_secret_key = ProtectedMemory::from_slice(secret_key).await?;
        let new_key_material =
            KeyMaterial::new(Some(new_secret_key), key_material.public_key).await?;

        new_key_material.serialize().await?
    };

    let new_encrypted = encrypt(master_key, &new_key_bytes, &metadata, crypto).await?;

    Ok(Some(new_encrypted))
}
