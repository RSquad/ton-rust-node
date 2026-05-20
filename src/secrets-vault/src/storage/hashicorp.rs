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
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    storage::{
        hashicorp_api::{Client, KeyMode, VaultConfig},
        hashicorp_token_provider::AuthConfig,
        storage_trait::Storage,
    },
    types::{
        algorithm::Algorithm,
        metadata::Metadata,
        payload::PayloadType,
        secret::{Blob, KeyPair, Secret},
        secret_id::SecretId,
        secret_spec::SecretSpec,
        store_mode::StoreMode,
    },
};
use std::sync::Arc;

pub(crate) struct KeyPairHashicorp {
    metadata: Metadata,
    client: Arc<Client>,
    key_material: KeyMaterial,
    crypto: Arc<dyn Crypto>,
    prefer_local_crypto: bool,
}

impl KeyPairHashicorp {
    pub async fn new(
        metadata: Metadata,
        client: Arc<Client>,
        crypto: Arc<dyn Crypto>,
        prefer_local_crypto: bool,
    ) -> anyhow::Result<Self> {
        let key_material = Self::load_keys(client.as_ref(), &metadata).await?;
        Ok(Self { metadata, client, key_material, crypto, prefer_local_crypto })
    }

    async fn load_keys(client: &Client, metadata: &Metadata) -> anyhow::Result<KeyMaterial> {
        let secret_id = metadata
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to load keys"))?;
        let (key_info, _) = client.get_key_info(secret_id.as_string()).await?;
        let public_key = base64::decode(key_info.public_key)?;
        let private_key = if metadata.extractable {
            let private_key = client.export_key(secret_id.as_str(), KeyMode::Signing).await?;
            Some(private_key)
        } else {
            None
        };

        KeyMaterial::new(private_key, Some(public_key))
    }
}

#[async_trait::async_trait]
impl KeyPair for KeyPairHashicorp {
    fn id(&self) -> Option<&SecretId> {
        self.metadata.secret_id.as_ref()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }

    fn extractable(&self) -> bool {
        self.metadata.extractable
    }

    fn public_key(&self) -> Option<&[u8]> {
        self.key_material.public_key.as_deref()
    }

    fn private_key(&self) -> anyhow::Result<&ProtectedMemory> {
        if !self.metadata.extractable {
            anyhow::bail!(VaultError::not_extractable(self.metadata.secret_id.as_ref()))
        }

        let pvt_key = self
            .key_material
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Private key is not set"))?;

        Ok(pvt_key)
    }

    fn expanded_key(&self) -> anyhow::Result<ProtectedMemory> {
        let pvt_key = self.private_key()?;
        let lock = &pvt_key.lock()?;
        let exp_key = self.crypto.exp_key_from_pvt(self.metadata.algorithm, lock)?;

        Ok(exp_key)
    }

    async fn sign(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if self.metadata.extractable && self.prefer_local_crypto {
            self.crypto.as_ref().sign(&self.key_material, data, self.metadata.algorithm)
        } else {
            let secret_id = self
                .metadata
                .secret_id
                .as_ref()
                .ok_or_else(|| VaultError::empty_secret_id("Failed to sign"))?;

            self.client.sign(secret_id.as_str(), data).await
        }
    }

    async fn verify(&self, data: &[u8], signature: &[u8]) -> anyhow::Result<()> {
        if self.metadata.extractable && self.prefer_local_crypto {
            self.crypto.verify(
                self.key_material
                    .public_key
                    .as_ref()
                    .ok_or_else(|| VaultError::empty_public_key("Failed to verify"))?,
                data,
                signature,
                self.metadata.algorithm,
            )
        } else {
            let secret_id = self
                .metadata
                .secret_id
                .as_ref()
                .ok_or_else(|| VaultError::empty_secret_id("Failed to verify"))?;

            self.client.verify_sign(secret_id.as_str(), data, signature).await
        }
    }

    fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        anyhow::bail!("Serialization is not supported for HashiCorp-backed secrets");
    }
}

pub struct BlobHashicorp {
    metadata: Metadata,
    data: ProtectedMemory,
}

impl BlobHashicorp {
    pub async fn new(metadata: Metadata, client: &Client) -> anyhow::Result<Self> {
        let data = Self::load_data(client, &metadata).await?;
        Ok(Self { metadata, data })
    }

    async fn load_data(client: &Client, metadata: &Metadata) -> anyhow::Result<ProtectedMemory> {
        let secret_id = metadata
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to read blob data"))?;
        let blob_data = client.read_blob(secret_id.as_str()).await?;
        let bytes = base64::decode(&blob_data.data)?;
        Ok(ProtectedMemoryInner::from_slice(&bytes)?.into())
    }
}

impl Blob for BlobHashicorp {
    fn id(&self) -> Option<&SecretId> {
        self.metadata.secret_id.as_ref()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn metadata_mut(&mut self) -> &mut Metadata {
        &mut self.metadata
    }

    fn data(&self) -> &ProtectedMemory {
        &self.data
    }
}

pub struct HashicorpStorage {
    client: Arc<Client>,
    crypto: Arc<dyn Crypto>,
    prefer_local_crypto: bool,
}

impl HashicorpStorage {
    pub fn new(
        auth: AuthConfig,
        url: &str,
        prefer_local_crypto: bool,
        crypto: Arc<dyn Crypto>,
        config: VaultConfig,
    ) -> anyhow::Result<Self> {
        let client = Client::new(url, auth, config)?;
        Ok(Self { client: Arc::new(client), crypto, prefer_local_crypto })
    }

    fn algorithm_to_key_type(algorithm: Algorithm) -> anyhow::Result<String> {
        match algorithm {
            Algorithm::None | Algorithm::Aes256Gcm => {
                anyhow::bail!(VaultError::unsupported_algorithm(algorithm));
            }
            Algorithm::Ed25519 => Ok("ed25519".to_string()),
        }
    }
}

#[async_trait::async_trait]
impl Storage for HashicorpStorage {
    async fn flush(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn generate_secret(
        &self,
        spec: &SecretSpec,
        secret_id: &SecretId,
    ) -> anyhow::Result<Secret> {
        let metadata = Metadata::from_spec(Some(secret_id), spec);

        match spec.algorithm {
            Algorithm::Aes256Gcm => {
                anyhow::bail!(VaultError::unsupported_algorithm(spec.algorithm));
            }
            Algorithm::Ed25519 => {
                let key_type = HashicorpStorage::algorithm_to_key_type(spec.algorithm)?;
                // TODO: fix non-atomic create_key + set_metadata
                // create_key() atomically rejects duplicates via the transit backend
                self.client.create_key(secret_id.as_string(), &key_type, spec.extractable).await?;
                // cas=0: only write metadata if it doesn't exist yet
                self.client.set_metadata(secret_id.as_str(), &metadata, Some(0)).await?;
                self.load(secret_id).await
            }
            Algorithm::None => {
                let key_material =
                    KeyMaterial::generate_new(spec.algorithm, spec.size, self.crypto.as_ref())?;

                let pvt_key = key_material
                    .secret_key
                    .as_ref()
                    .ok_or_else(|| VaultError::empty_secret_key("Failed to generate secret"))?;

                let data_b64 = base64::encode(&pvt_key.lock()?);
                // TODO: fix non-atomic write_blob + set_metadata
                // write_blob() with NewOnly uses cas=0 internally for atomic create
                self.client
                    .write_blob(secret_id.as_str(), &data_b64, StoreMode::NewOnly, None)
                    .await?;
                // cas=0: only write metadata if it doesn't exist yet.
                self.client.set_metadata(secret_id.as_str(), &metadata, Some(0)).await?;
                self.load(secret_id).await
            }
        }
    }

    async fn store(
        &self,
        secret: &Secret,
        mode: StoreMode,
        override_extractable: Option<bool>,
    ) -> anyhow::Result<()> {
        let mut metadata = secret.metadata().clone();
        if let Some(override_extractable) = override_extractable {
            metadata.extractable = override_extractable;
        }

        let secret_id = metadata
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to store secret"))?
            .to_string();

        match metadata.algorithm {
            Algorithm::None => {
                let data_b64 = {
                    let blob = secret.as_blob()?;
                    let data = blob.data();
                    let lock = data.lock()?;
                    base64::encode(&*lock)
                };

                // TODO: fix non-atomic write_blob + set_metadata
                self.client.write_blob(&secret_id, &data_b64, mode, None).await?;
                self.client.set_metadata(&secret_id, &metadata, None).await?;
            }
            Algorithm::Ed25519 => {
                // TODO: fix non-atomic import_secret_ed25519 + set_metadata
                self.client.import_secret_ed25519(secret, mode).await?;
                self.client.set_metadata(&secret_id, &metadata, None).await?;
            }
            _ => {
                anyhow::bail!(VaultError::unsupported_algorithm(metadata.algorithm));
            }
        }

        Ok(())
    }

    async fn load(&self, secret_id: &SecretId) -> anyhow::Result<Secret> {
        let metadata = self.load_metadata(secret_id).await?.ok_or_else(|| {
            VaultError::not_found(format!("Metadata with id '{}' not found", secret_id))
        })?;
        let secret = match metadata.algorithm.payload_type() {
            PayloadType::Blob => Secret::Blob {
                blob: Box::new(BlobHashicorp::new(metadata, self.client.as_ref()).await?),
            },
            PayloadType::KeyPair => Secret::KeyPair {
                keypair: Box::new(
                    KeyPairHashicorp::new(
                        metadata,
                        self.client.clone(),
                        self.crypto.clone(),
                        self.prefer_local_crypto,
                    )
                    .await?,
                ),
            },
            PayloadType::SymmetricKey => {
                anyhow::bail!(VaultError::unsupported_algorithm(metadata.algorithm))
            }
        };

        Ok(secret)
    }

    async fn delete(&self, secret_id: &SecretId) -> anyhow::Result<()> {
        let metadata = self.client.get_metadata(secret_id.as_str()).await?.ok_or_else(|| {
            anyhow::anyhow!(VaultError::empty_metadata(format!(
                "Metadata with id '{}' not found",
                secret_id
            )))
        })?;

        if metadata.is_blob() {
            self.client.delete_blob(secret_id.as_str()).await?;
        } else {
            self.client.delete_key(secret_id.as_str()).await?;
        }

        self.client.delete_metadata(secret_id.as_str()).await?;

        Ok(())
    }

    async fn list_metadata(&self) -> anyhow::Result<Vec<Metadata>> {
        let mut metas = Vec::new();

        let names = self.client.list_meta().await?;
        for secret_id in names {
            let meta = self.load_metadata(&secret_id.as_str().into()).await?;
            if let Some(m) = meta {
                metas.push(m);
            }
        }

        Ok(metas)
    }

    async fn load_metadata(&self, secret_id: &SecretId) -> anyhow::Result<Option<Metadata>> {
        let metadata = self.client.get_metadata(secret_id.as_str()).await?;
        Ok(metadata)
    }

    fn format_version(&self) -> anyhow::Result<u32> {
        Ok(1)
    }

    #[cfg(test)]
    async fn clear(&self) -> anyhow::Result<()> {
        // KV: hard-delete every leaf under blobs/ and meta/.
        for subdir in ["blobs", "meta"] {
            let names = self.client.list_kv_subdir(subdir).await?;
            for name in names {
                if name.ends_with('/') {
                    continue;
                }
                self.client.kv_hard_delete(&format!("{subdir}/{name}")).await?;
            }
        }

        // Transit: hard-delete every key in the mount.
        let keys = self.client.list_all_mount_transit_keys().await?;
        for key in keys {
            self.client.delete_key(&key).await?;
        }

        Ok(())
    }

    #[cfg(test)]
    async fn is_empty(&self) -> anyhow::Result<bool> {
        for subdir in ["blobs", "meta"] {
            if !self.client.list_kv_subdir(subdir).await?.is_empty() {
                return Ok(false);
            }
        }
        Ok(self.client.list_all_mount_transit_keys().await?.is_empty())
    }
}
