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
        crypto_trait::{
            Crypto, CryptoImpl, Ed25519Backend, ED25519_EXPANDED_KEY_LENGTH,
            ED25519_PRIVATE_KEY_LENGTH, ED25519_PUBLIC_KEY_LENGTH,
        },
        factory::CryptoFactory,
        key_material::KeyMaterial,
        prng_chacha20::PrngChacha20,
    },
    errors::error::VaultError,
    make_secret_id,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    types::{
        algorithm::Algorithm,
        metadata::Metadata,
        secret::{Secret, SecretDataType, SecretInMemoryFactory},
        secret_id::SecretId,
    },
    vault::SecretVault,
    vault_builder::SecretVaultBuilder,
};
use std::sync::Arc;
use ton_block::{
    base64_encode, ed25519_create_expanded_private_key, ed25519_create_private_key,
    ed25519_create_public_key, ed25519_expand_private_key, ed25519_generate_private_key,
    ed25519_sign, ed25519_verify, error, fail, sha256_digest_slices, x25519_shared_secret,
    Ed25519KeyOption, KeyId, KeyOption, KeyOptionJson, Result, SecretBytes, ZeroizingBytes,
    ED25519_KEY_TYPE,
};
use zeroize::Zeroize;

pub fn tokio_run<F>(fut: F) -> F::Output
where
    F: std::future::Future + Send,
    F::Output: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => std::thread::scope(|s| {
            s.spawn(|| handle.block_on(fut)).join().expect("tokio_run thread panicked")
        }),
        Err(_) => {
            tokio::runtime::Runtime::new().expect("failed to create Tokio runtime").block_on(fut)
        }
    }
}
pub struct BlockEd25519;

impl Ed25519Backend for BlockEd25519 {
    fn sign_ed25519(key: &KeyMaterial, data: &[u8]) -> anyhow::Result<Vec<u8>> {
        let secret_key = key
            .secret_key
            .as_ref()
            .ok_or_else(|| VaultError::empty_secret_key("Secret key is not set"))?;

        let key_lock = secret_key.lock()?;
        let is_expanded =
            key.tags.as_ref().is_some_and(|t| t.iter().any(|t| t == KeyMaterial::TAG_EXPANDED_KEY));

        if is_expanded {
            if key_lock.len() != ED25519_EXPANDED_KEY_LENGTH {
                anyhow::bail!(VaultError::invalid_key_size(format!(
                    "Invalid expanded key size for Ed25519, expected {}, got {}",
                    ED25519_EXPANDED_KEY_LENGTH,
                    key_lock.len()
                )));
            }
            ed25519_sign(&key_lock, None, data)
        } else {
            if key_lock.len() != ED25519_PRIVATE_KEY_LENGTH {
                anyhow::bail!(VaultError::invalid_key_size(format!(
                    "Invalid key size for Ed25519, expected {}, got {}",
                    ED25519_PRIVATE_KEY_LENGTH,
                    key_lock.len()
                )));
            }
            let pvt_key_exp = ed25519_expand_private_key(&ed25519_create_private_key(&key_lock)?)?;
            ed25519_sign(pvt_key_exp.to_bytes().as_ref(), None, data)
        }
    }

    fn verify_ed25519(
        pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH],
        data: &[u8],
        signature: &[u8],
    ) -> anyhow::Result<()> {
        ed25519_verify(pub_key, data, signature)
            .map_err(|_| VaultError::invalid_signature("Signature verification failed"))?;
        Ok(())
    }

    fn pub_key_from_pvt(pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH]) -> anyhow::Result<Vec<u8>> {
        let pvt_key = ed25519_create_private_key(pvt_key)?;
        let pvt_key_exp = ed25519_expand_private_key(&pvt_key)?;
        let pub_key = ed25519_create_public_key(&pvt_key_exp)?;
        Ok(pub_key.as_bytes().to_vec())
    }

    fn pub_key_from_exp(exp_key: &[u8; ED25519_EXPANDED_KEY_LENGTH]) -> anyhow::Result<Vec<u8>> {
        let pub_key = ed25519_create_public_key(&ed25519_create_expanded_private_key(exp_key)?)?;
        Ok(pub_key.as_bytes().to_vec())
    }

    fn exp_key_from_pvt(
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> anyhow::Result<ProtectedMemory> {
        let exp_key = ed25519_expand_private_key(&ed25519_create_private_key(pvt_key)?)?;
        Ok(ProtectedMemory::from(ProtectedMemoryInner::from_slice(exp_key.as_bytes())?))
    }
}

#[derive(Debug)]
pub struct BlockCryptoFactory {}

impl CryptoFactory for BlockCryptoFactory {
    fn new_crypto(&self) -> anyhow::Result<Arc<dyn Crypto>> {
        let prng = Box::new(PrngChacha20 {});
        Ok(Arc::new(CryptoImpl::<BlockEd25519>::new(prng)))
    }
}

#[derive(Debug)]
pub struct VaultKeyOption {
    id: Arc<KeyId>,
    secret: Secret,
    // Eagerly cached expanded key: KeyOption::exp_key returns by-ref now.
    // None for non-extractable / public-only secrets.
    exp_key: Option<ProtectedMemory>,
}

impl VaultKeyOption {
    pub fn new(id: Arc<KeyId>, secret: Secret) -> Result<Self> {
        let exp_key = secret.as_keypair().ok().and_then(|kp| kp.expanded_key().ok());
        Ok(Self { id, secret, exp_key })
    }

    pub fn secret(&self) -> &Secret {
        &self.secret
    }
}

impl KeyOption for VaultKeyOption {
    /// Get key id
    fn id(&self) -> &Arc<KeyId> {
        &self.id
    }

    /// Get type id
    fn type_id(&self) -> i32 {
        ED25519_KEY_TYPE
    }

    /// Get public key
    fn pub_key(&self) -> Result<&[u8]> {
        if let Some(pub_key) = self.secret.as_keypair()?.public_key() {
            Ok(pub_key)
        } else {
            fail!("No public key set for key option {}", self.id())
        }
    }

    /// Get private key
    fn pvt_key(&self) -> Result<&dyn SecretBytes> {
        Ok(self.secret.as_keypair()?.private_key()?)
    }

    /// Get expanded key
    fn exp_key(&self) -> Result<&dyn SecretBytes> {
        self.exp_key
            .as_ref()
            .map(|k| k as &dyn SecretBytes)
            .ok_or_else(|| error!("No expansion key set for key option {}", self.id()))
    }

    /// Calculate signature
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        let sign_res: Result<Vec<u8>> =
            tokio_run(async { self.secret.as_keypair()?.sign(data).await });

        sign_res
    }

    /// Verify signature
    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<()> {
        let verify_res: Result<()> =
            tokio_run(async { self.secret.as_keypair()?.verify(data, signature).await });

        verify_res
    }

    /// Calculate shared secret
    fn shared_secret(&self, other_pub_key: &[u8]) -> Result<Box<dyn SecretBytes>> {
        let mut shared = x25519_shared_secret(&self.exp_key()?.lock()?, other_pub_key)?;
        let result = ProtectedMemory::from(ProtectedMemoryInner::from_slice(&shared)?);
        shared.zeroize();
        Ok(Box::new(result))
    }
}

#[derive(Debug)]
pub struct VaultKeyOptionFactory {
    vault: Option<Arc<SecretVault>>,
}

impl VaultKeyOptionFactory {
    pub fn new() -> Result<Self> {
        let vault = tokio_run(SecretVaultBuilder::from_url_or_env(
            None,
            BlockCryptoFactory {}.new_crypto()?,
        ))?;
        Ok(Self { vault })
    }

    /// Create from Ed25519 expanded secret key raw data
    pub fn from_expanded_key(
        &self,
        exp_key: &[u8; ED25519_EXPANDED_KEY_LENGTH],
    ) -> Result<Arc<dyn KeyOption>> {
        if self.vault.is_some() {
            let mut secret = Self::secret_from_raw(exp_key, SecretDataType::Ed25519ExpKey)?;
            let id = Self::key_id_from_secret(&secret)?;
            secret.metadata_mut().secret_id = Some(make_secret_id!(base64_encode(id.data())));
            Ok(Arc::new(VaultKeyOption::new(id, secret)?))
        } else {
            Ed25519KeyOption::<ProtectedMemory>::from_expanded_key(exp_key)
        }
    }

    /// Create from Ed25519 secret key raw data
    pub fn from_private_key(
        &self,
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> Result<Arc<dyn KeyOption>> {
        if self.vault.is_some() {
            let mut secret = Self::secret_from_raw(pvt_key, SecretDataType::Ed25519PvtKey)?;
            let id = Self::key_id_from_secret(&secret)?;
            secret.metadata_mut().secret_id = Some(make_secret_id!(base64_encode(id.data())));
            Ok(Arc::new(VaultKeyOption::new(id, secret)?))
        } else {
            Ed25519KeyOption::<ProtectedMemory>::from_private_key(pvt_key)
        }
    }

    /// Create from Ed25519 secret key raw data and export JSON
    pub fn from_private_key_with_json(
        &self,
        pvt_key: &[u8; ED25519_PRIVATE_KEY_LENGTH],
    ) -> Result<(KeyOptionJson, Arc<dyn KeyOption>)> {
        if self.vault.is_some() {
            let mut secret = Self::secret_from_raw(pvt_key, SecretDataType::Ed25519PvtKey)?;
            let id = Self::key_id_from_secret(&secret)?;
            let secret_id = make_secret_id!(base64_encode(id.data()));
            let name = secret_id.to_string();
            secret.metadata_mut().secret_id = Some(secret_id);
            let config = KeyOptionJson {
                type_id: ED25519_KEY_TYPE,
                pub_key: None,
                pvt_key: None,
                vault: Some(name),
            };
            Ok((config, Arc::new(VaultKeyOption::new(id, secret)?)))
        } else {
            Ed25519KeyOption::<ProtectedMemory>::from_private_key_with_json(pvt_key)
        }
    }

    /// Create from Ed25519 secret key JSON
    pub fn from_private_key_json(&self, config: &KeyOptionJson) -> Result<Arc<dyn KeyOption>> {
        let Some(name) = &config.vault else {
            return Ed25519KeyOption::<ProtectedMemory>::from_private_key_json(config);
        };
        let res: Result<(Arc<KeyId>, Secret)> = tokio_run(async {
            if let Some(vault) = self.vault.as_deref() {
                let loaded = vault.load(&SecretId::from(name.clone())).await?;
                let secret = match loaded {
                    Secret::KeyPair { .. } => loaded,
                    Secret::Blob { blob } => {
                        Self::secret_from_raw(&blob.data().lock()?, SecretDataType::Ed25519PvtKey)?
                    }
                    _ => fail!("Unsupported secret type for key '{name}': {loaded}"),
                };
                let id = Self::key_id_from_secret(&secret)?;
                Ok((id, secret))
            } else {
                fail!("failed to load key '{name}' from Vault: Vault is not connected")
            }
        });
        let (id, secret) = res?;
        Ok(Arc::new(VaultKeyOption::new(id, secret)?))
    }

    /// Create from Ed25519 public key raw data
    pub fn from_public_key(&self, pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH]) -> Arc<dyn KeyOption> {
        Arc::new(Ed25519KeyOption::<ZeroizingBytes>::new(
            Self::key_id(ED25519_KEY_TYPE, pub_key),
            Some(*pub_key),
            None,
            None,
        ))
    }

    /// Create from Ed25519 public key JSON
    pub fn from_public_key_json(&self, config: &KeyOptionJson) -> Result<Arc<dyn KeyOption>> {
        let Some(name) = &config.vault else {
            return Ed25519KeyOption::<ZeroizingBytes>::from_public_key_json(config);
        };
        let res: Result<(Arc<KeyId>, Secret)> = tokio_run(async {
            if let Some(vault) = self.vault.as_deref() {
                let secret = vault.load(&SecretId::from(name.clone())).await?;
                let id = Self::key_id_from_secret(&secret)?;
                Ok((id, secret))
            } else {
                fail!("failed to load key '{name}' from Vault: Vault is not connected")
            }
        });
        let (id, secret) = res?;
        Ok(Arc::new(VaultKeyOption::new(id, secret)?))
    }

    /// Generate new Ed25519 key
    pub fn generate(&self) -> Result<Arc<dyn KeyOption>> {
        self.from_private_key(ed25519_generate_private_key()?.as_bytes().try_into()?)
    }

    /// Generate new Ed25519 key and export JSON
    pub fn generate_with_json(&self) -> Result<(KeyOptionJson, Arc<dyn KeyOption>)> {
        self.from_private_key_with_json(ed25519_generate_private_key()?.as_bytes().try_into()?)
    }

    fn key_id(type_id: i32, pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH]) -> Arc<KeyId> {
        let data = sha256_digest_slices(&[&type_id.to_le_bytes(), pub_key]);
        KeyId::from_data(data)
    }

    fn key_id_from_secret(secret: &Secret) -> Result<Arc<KeyId>> {
        Ok(Self::key_id(
            ED25519_KEY_TYPE,
            secret
                .as_keypair()?
                .public_key()
                .ok_or_else(|| VaultError::empty_public_key("Failed to get public key"))?
                .try_into()?,
        ))
    }

    fn secret_from_raw(data: &[u8], data_type: SecretDataType) -> Result<Secret> {
        Ok(SecretInMemoryFactory::from_raw_data(
            data,
            data_type,
            Metadata::new(None, Algorithm::Ed25519, true),
            BlockCryptoFactory {}.new_crypto()?,
        )?)
    }
}

static FACTORY: std::sync::OnceLock<Box<VaultKeyOptionFactory>> = std::sync::OnceLock::new();

pub fn get_key_option_factory() -> &'static VaultKeyOptionFactory {
    FACTORY.get_or_init(|| {
        let res = VaultKeyOptionFactory::new()
            .unwrap_or_else(|e| panic!("failed to call get_key_option_factory(): {}", e));

        Box::new(res)
    })
}
