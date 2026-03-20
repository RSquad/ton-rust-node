/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    crypto::{crypto_trait::Crypto, factory::CryptoFactory, key_material::KeyMaterial},
    errors::error::VaultError,
    memory::protected_memory::ProtectedMemory,
    storage::{
        hashicorp_api::{Client, KeyMode},
        storage_trait::Storage,
        utils,
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
    crypto: Option<Box<dyn Crypto>>,
    prefer_local_crypto: bool,
}

impl KeyPairHashicorp {
    pub async fn new(
        metadata: Metadata,
        client: Arc<Client>,
        crypto: Option<Box<dyn Crypto>>,
        prefer_local_crypto: bool,
    ) -> anyhow::Result<Self> {
        let key_material = Self::load_keys(client.as_ref(), &metadata, prefer_local_crypto).await?;
        Ok(Self { metadata, client, key_material, crypto, prefer_local_crypto })
    }

    async fn load_keys(
        client: &Client,
        metadata: &Metadata,
        prefer_local_crypto: bool,
    ) -> anyhow::Result<KeyMaterial> {
        let secret_id = metadata
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to load keys"))?;
        let (key_info, _) = client.get_key_info(secret_id.as_string()).await?;
        let public_key = utils::b64::decode(key_info.public_key)?;
        let private_key = if metadata.extractable && prefer_local_crypto {
            let private_key = client.export_key(secret_id.as_str(), KeyMode::Signing).await?;
            Some(private_key)
        } else {
            None
        };

        KeyMaterial::new(private_key, Some(public_key)).await
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

    async fn extractable(&self) -> anyhow::Result<bool> {
        Ok(self.metadata.extractable)
    }

    async fn public_key(&self) -> anyhow::Result<Option<Vec<u8>>> {
        let pub_key = self
            .key_material
            .public_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_public_key("Failed to get public key"))?;
        Ok(Some(pub_key.clone()))
    }

    async fn private_key(&self) -> anyhow::Result<ProtectedMemory> {
        if !self.metadata.extractable {
            anyhow::bail!(VaultError::not_extractable(self.metadata.secret_id.as_ref()))
        }

        if let Some(cached_key) = self.key_material.secret_key.as_ref() {
            return cached_key.clone().await;
        }

        // Key not cached — fetch on demand from HashiCorp Vault
        let secret_id = self
            .metadata
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to get private key"))?;
        self.client.export_key(secret_id.as_str(), KeyMode::Signing).await
    }

    async fn sign(&self, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        if self.metadata.extractable && self.prefer_local_crypto {
            self.crypto
                .as_ref()
                .ok_or_else(|| VaultError::empty_crypto("Failed to sign"))?
                .sign(&self.key_material, data, self.metadata.algorithm)
                .await
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
            self.crypto
                .as_ref()
                .ok_or_else(|| VaultError::empty_crypto("Failed to verify"))?
                .verify(
                    self.key_material
                        .public_key
                        .as_ref()
                        .ok_or_else(|| VaultError::empty_public_key("Failed to verify"))?,
                    data,
                    signature,
                    self.metadata.algorithm,
                )
                .await
        } else {
            let secret_id = self
                .metadata
                .secret_id
                .as_ref()
                .ok_or_else(|| VaultError::empty_secret_id("Failed to verify"))?;

            self.client.verify_sign(secret_id.as_str(), data, signature).await
        }
    }

    async fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        anyhow::bail!("Serialization is not supported for HashiCorp-backed secrets");
    }
}

pub struct BlobHashicorp {
    metadata: Metadata,
    client: Arc<Client>,
}

impl BlobHashicorp {
    pub fn new(metadata: Metadata, client: Arc<Client>) -> Self {
        Self { metadata, client }
    }
}

#[async_trait::async_trait]
impl Blob for BlobHashicorp {
    fn id(&self) -> Option<&SecretId> {
        self.metadata.secret_id.as_ref()
    }

    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    async fn data(&self) -> anyhow::Result<ProtectedMemory> {
        let secret_id = self
            .metadata
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to read blob data"))?;
        let blob_data = self.client.read_blob(secret_id.as_str()).await?;
        let bytes = utils::b64::decode(&blob_data.data)?;
        ProtectedMemory::from_slice(&bytes).await
    }
}

pub struct HashicorpStorage {
    client: Arc<Client>,
    crypto_factory: Box<dyn CryptoFactory>,
    prefer_local_crypto: bool,
}

impl HashicorpStorage {
    pub async fn new(
        api_key: ProtectedMemory,
        url: &str,
        namespace: Option<&str>,
        crypto_factory: Box<dyn CryptoFactory>,
        prefer_local_crypto: bool,
    ) -> anyhow::Result<Self> {
        let mut client = Client::new(url, api_key)?;
        if let Some(namespace) = namespace {
            client = client.with_namespace(namespace);
        }
        Ok(Self { client: Arc::new(client), crypto_factory, prefer_local_crypto })
    }

    pub async fn migrate() -> anyhow::Result<()> {
        Ok(())
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
                // create_key() atomically rejects duplicates via the transit backend
                self.client.create_key(secret_id.as_string(), &key_type, spec.extractable).await?;
                // cas=0: only write metadata if it doesn't exist yet
                self.client.set_metadata(secret_id.as_str(), &metadata, Some(0)).await?;
                self.load(secret_id).await
            }
            Algorithm::None => {
                let key_material = KeyMaterial::generate_new(
                    spec.algorithm,
                    spec.size,
                    self.crypto_factory.new_crypto()?.as_ref(),
                )
                .await?;

                let pvt_key = key_material
                    .secret_key
                    .as_ref()
                    .ok_or_else(|| VaultError::empty_secret_key("Failed to generate secret"))?;

                let data_b64 = utils::b64::encode(&pvt_key.lock().await?);
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

    async fn store(&self, secret: &Secret, mode: StoreMode) -> anyhow::Result<()> {
        let metadata = secret.metadata();
        let secret_id = metadata
            .secret_id
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_id("Failed to store secret"))?
            .to_string();

        match metadata.algorithm {
            Algorithm::None => {
                let blob = secret.as_blob()?;
                let data = blob.data().await?;
                let lock = data.lock().await?;
                let data_b64 = utils::b64::encode(&*lock);

                self.client.write_blob(&secret_id, &data_b64, mode, None).await?;
                self.client.set_metadata(&secret_id, metadata, None).await?;
            }
            Algorithm::Ed25519 => {
                self.client.import_secret_ed25519(secret, mode).await?;
                self.client.set_metadata(&secret_id, metadata, None).await?;
            }
            _ => {
                anyhow::bail!(VaultError::unsupported_algorithm(metadata.algorithm));
            }
        }

        Ok(())
    }

    async fn store_vec(
        &self,
        _secrets: Vec<(ProtectedMemory, Metadata, StoreMode)>,
    ) -> anyhow::Result<()> {
        // TODO: implement
        anyhow::bail!("store_vec is not implemented")
    }

    async fn load(&self, secret_id: &SecretId) -> anyhow::Result<Secret> {
        let metadata = self.load_metadata(secret_id).await?.ok_or_else(|| {
            VaultError::not_found(format!("Metadata with id '{}' not found", secret_id))
        })?;
        let secret = match metadata.algorithm.payload_type() {
            PayloadType::Blob => {
                Secret::Blob { blob: Box::new(BlobHashicorp::new(metadata, self.client.clone())) }
            }
            PayloadType::KeyPair => {
                let crypto = if metadata.extractable && self.prefer_local_crypto {
                    Some(self.crypto_factory.new_crypto()?)
                } else {
                    None
                };

                Secret::KeyPair {
                    keypair: Box::new(
                        KeyPairHashicorp::new(
                            metadata,
                            self.client.clone(),
                            crypto,
                            self.prefer_local_crypto,
                        )
                        .await?,
                    ),
                }
            }
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

        // Transit keys
        let transit_keys = self.client.list_keys().await?;
        for secret_id in transit_keys {
            let meta = self.load_metadata(&secret_id.as_str().into()).await?;

            if let Some(m) = meta {
                metas.push(m);
            }
        }

        // KV blobs
        let blob_names = self.client.list_blobs().await?;
        for secret_id in blob_names {
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
}
