/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    base64_decode, base64_encode, ed25519_create_expanded_private_key, ed25519_create_private_key,
    ed25519_create_public_key, ed25519_expand_private_key, ed25519_generate_private_key,
    ed25519_sign, ed25519_verify, fail, sha256_digest_slices, x25519_shared_secret, Result,
    ED25519_EXPANDED_KEY_LENGTH, ED25519_PUBLIC_KEY_LENGTH, ED25519_SECRET_KEY_LENGTH,
};
use std::{
    fmt::{self, Debug, Display, Formatter},
    ops::Deref,
    sync::Arc,
};
use zeroize::Zeroize;

/// Interface to cryptographic keys
pub trait KeyOption: Sync + Send {
    fn id(&self) -> &Arc<KeyId>;
    fn type_id(&self) -> i32;
    fn pub_key(&self) -> Result<&[u8]>;
    fn pvt_key(&self) -> Result<&dyn SecretBytes>;
    fn exp_key(&self) -> Result<&dyn SecretBytes>;
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>>;
    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<()>;
    fn shared_secret(&self, other_pub_key: &[u8]) -> Result<Box<dyn SecretBytes>>;
}

impl Debug for dyn KeyOption {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyOption").field("id", self.id()).finish_non_exhaustive()
    }
}

/// Backend-agnostic secret byte container
pub trait SecretBytes: Send + Sync {
    fn lock(&self) -> Result<SecretBytesReadGuard<'_>>;
    fn from_slice(data: &[u8]) -> Result<Self>
    where
        Self: Sized;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Ed25519 key interface
pub struct Ed25519KeyOption<S: SecretBytes> {
    id: Arc<KeyId>,
    pub_key: Option<[u8; ED25519_PUBLIC_KEY_LENGTH]>,
    exp_key: Option<S>,
    pvt_key: Option<S>,
}

pub const ED25519_KEY_TYPE: i32 = 1209251014;

impl<S: SecretBytes + 'static> Ed25519KeyOption<S> {
    pub fn new(
        id: Arc<KeyId>,
        pub_key: Option<[u8; ED25519_PUBLIC_KEY_LENGTH]>,
        exp_key: Option<S>,
        pvt_key: Option<S>,
    ) -> Self {
        Self { id, pub_key, exp_key, pvt_key }
    }

    fn key_id(type_id: i32, pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH]) -> Arc<KeyId> {
        let data = sha256_digest_slices(&[&type_id.to_le_bytes(), pub_key]);
        KeyId::from_data(data)
    }

    /// Create from Ed25519 expanded secret key raw data
    pub fn from_expanded_key(
        exp_key: &[u8; ED25519_EXPANDED_KEY_LENGTH],
    ) -> Result<Arc<dyn KeyOption>> {
        let pub_key =
            ed25519_create_public_key(&ed25519_create_expanded_private_key(exp_key)?)?.to_bytes();

        Ok(Arc::new(Ed25519KeyOption::<S> {
            id: Self::key_id(ED25519_KEY_TYPE, &pub_key),
            pub_key: Some(pub_key),
            exp_key: Some(S::from_slice(exp_key)?),
            pvt_key: None,
        }))
    }

    /// Create from Ed25519 secret key raw data
    pub fn from_private_key(
        pvt_key: &[u8; ED25519_SECRET_KEY_LENGTH],
    ) -> Result<Arc<dyn KeyOption>> {
        let exp_key = ed25519_expand_private_key(&ed25519_create_private_key(pvt_key)?)?;
        let pub_key = ed25519_create_public_key(&exp_key)?.to_bytes();

        Ok(Arc::new(Ed25519KeyOption::<S> {
            id: Self::key_id(ED25519_KEY_TYPE, &pub_key),
            pub_key: Some(pub_key),
            exp_key: Some(S::from_slice(exp_key.as_bytes())?),
            pvt_key: Some(S::from_slice(pvt_key)?),
        }))
    }

    /// Create from Ed25519 secret key raw data and export JSON
    pub fn from_private_key_with_json(
        pvt_key: &[u8; ED25519_SECRET_KEY_LENGTH],
    ) -> Result<(KeyOptionJson, Arc<dyn KeyOption>)> {
        let key_opt = Self::from_private_key(pvt_key)?;

        let config = KeyOptionJson {
            type_id: ED25519_KEY_TYPE,
            pub_key: None,
            pvt_key: Some(base64_encode(pvt_key)),
            vault: None,
        };

        Ok((config, key_opt))
    }

    /// Create from Ed25519 secret key JSON
    pub fn from_private_key_json(config: &KeyOptionJson) -> Result<Arc<dyn KeyOption>> {
        if config.vault.is_some() {
            fail!("Vault-backed key is not supported by Ed25519KeyOption");
        }
        if config.type_id != ED25519_KEY_TYPE {
            fail!("Type-id {} is not supported for Ed25519 private key", config.type_id);
        }
        let Some(pvt_key) = &config.pvt_key else {
            fail!("No private key");
        };
        if config.pub_key.is_some() {
            fail!("No public key expected");
        }
        let key = base64_decode(pvt_key)?;
        if key.len() != ED25519_SECRET_KEY_LENGTH {
            fail!("Bad private key");
        }
        Self::from_private_key(key.as_slice().try_into()?)
    }

    /// Create from Ed25519 public key raw data
    pub fn from_public_key(pub_key: &[u8; ED25519_PUBLIC_KEY_LENGTH]) -> Arc<dyn KeyOption> {
        Arc::new(Ed25519KeyOption::<S> {
            id: Self::key_id(ED25519_KEY_TYPE, pub_key),
            pub_key: Some(*pub_key),
            exp_key: None,
            pvt_key: None,
        })
    }

    /// Create from Ed25519 public key JSON
    pub fn from_public_key_json(config: &KeyOptionJson) -> Result<Arc<dyn KeyOption>> {
        if config.vault.is_some() {
            fail!("Vault-backed key is not supported by Ed25519KeyOption");
        }
        if config.type_id != ED25519_KEY_TYPE {
            fail!("Type-id {} is not supported for Ed25519 public key", config.type_id);
        }
        let Some(pub_key) = &config.pub_key else {
            fail!("No public key");
        };
        if config.pvt_key.is_some() {
            fail!("No private key expected");
        }
        let key = base64_decode(pub_key)?;
        if key.len() != ED25519_PUBLIC_KEY_LENGTH {
            fail!("Bad public key");
        }
        Ok(Self::from_public_key(key.as_slice().try_into()?))
    }

    /// Generate new Ed25519 key
    pub fn generate() -> Result<Arc<dyn KeyOption>> {
        Self::from_private_key(ed25519_generate_private_key()?.as_bytes().try_into()?)
    }

    /// Generate new Ed25519 key and export config
    pub fn generate_with_json() -> Result<(KeyOptionJson, Arc<dyn KeyOption>)> {
        Self::from_private_key_with_json(ed25519_generate_private_key()?.as_bytes().try_into()?)
    }
}

impl<S: SecretBytes + 'static> KeyOption for Ed25519KeyOption<S> {
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
        if let Some(pub_key) = self.pub_key.as_ref() {
            Ok(pub_key)
        } else {
            fail!("No public key set for key option {}", self.id())
        }
    }

    fn pvt_key(&self) -> Result<&dyn SecretBytes> {
        if let Some(pvt_key) = self.pvt_key.as_ref() {
            Ok(pvt_key)
        } else {
            fail!("No private key set for key option {}", self.id())
        }
    }

    fn exp_key(&self) -> Result<&dyn SecretBytes> {
        if let Some(exp_key) = self.exp_key.as_ref() {
            Ok(exp_key)
        } else {
            fail!("No expansion key set for key option {}", self.id())
        }
    }

    /// Calculate signature
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        ed25519_sign(&self.exp_key()?.lock()?, self.pub_key().ok(), data)
    }

    /// Verify signature
    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<()> {
        ed25519_verify(self.pub_key()?, data, signature)
    }

    /// Calculate shared secret
    fn shared_secret(&self, other_pub_key: &[u8]) -> Result<Box<dyn SecretBytes>> {
        let mut shared = x25519_shared_secret(&self.exp_key()?.lock()?, other_pub_key)?;
        let result = S::from_slice(&shared)?;
        shared.zeroize();
        Ok(Box::new(result))
    }
}

/// ADNL key ID (node ID)
#[derive(Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub struct KeyId([u8; 32]);

impl KeyId {
    pub fn from_data(data: [u8; 32]) -> Arc<Self> {
        Arc::new(Self(data))
    }
    pub fn data(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Display for KeyId {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{}", base64_encode(self.data()))
    }
}

#[derive(Clone, serde::Deserialize, serde::Serialize)]
pub struct KeyOptionJson {
    pub type_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pub_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pvt_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vault: Option<String>,
}

/// RAII secret bytes read guard; derefs to `&[u8]`.
pub enum SecretBytesReadGuard<'a> {
    Slice(&'a [u8]),
    Owned(Box<dyn Deref<Target = [u8]> + Send + 'a>),
}

impl Deref for SecretBytesReadGuard<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Slice(s) => s,
            Self::Owned(b) => b,
        }
    }
}

/// Default backend: zeroize-on-drop heap allocation.
pub type ZeroizingBytes = zeroize::Zeroizing<Vec<u8>>;

impl SecretBytes for ZeroizingBytes {
    fn lock(&self) -> Result<SecretBytesReadGuard<'_>> {
        Ok(SecretBytesReadGuard::Slice(self.as_slice()))
    }
    fn from_slice(data: &[u8]) -> Result<Self> {
        Ok(zeroize::Zeroizing::new(data.to_vec()))
    }
    fn len(&self) -> usize {
        self.as_slice().len()
    }
}
