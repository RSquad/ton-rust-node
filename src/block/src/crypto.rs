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
    ed25519_sign, ed25519_verify, fail, sha256_digest_slices, x25519_shared_secret,
    Ed25519ExpandedPrivateKey, Ed25519PrivateKey, Result,
};
use std::{
    convert::TryInto,
    fmt::{self, Debug, Display, Formatter},
    sync::Arc,
};

pub trait KeyOption: Sync + Send + Debug {
    fn id(&self) -> &Arc<KeyId>;
    fn type_id(&self) -> i32;
    fn pub_key(&self) -> Result<&[u8]>;
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>>;
    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<()>;
    #[cfg(feature = "export_key")]
    fn export_key(&self) -> Result<&[u8]>;
    /// Return the 32-byte Ed25519 private key if available.
    fn pvt_key(&self) -> Result<&[u8; 32]>;
    fn shared_secret(&self, other_pub_key: &[u8]) -> Result<[u8; 32]>;
}

#[derive(Debug)]
pub struct Ed25519KeyOption {
    id: Arc<KeyId>,
    pub_key: Option<[u8; Self::PUB_KEY_SIZE]>,
    exp_key: Option<[u8; Self::EXP_KEY_SIZE]>,
    /// 32-byte Ed25519 private key (when created via `from_private_key`).
    pvt_key: Option<[u8; Self::PVT_KEY_SIZE]>,
}

impl Ed25519KeyOption {
    pub const KEY_TYPE: i32 = 1209251014;
    pub const EXP_KEY_SIZE: usize = 64;
    pub const PVT_KEY_SIZE: usize = 32;
    pub const PUB_KEY_SIZE: usize = 32;

    /// Create from Ed25519 expanded secret key raw data
    pub fn from_expanded_key(exp_key: &[u8; Self::EXP_KEY_SIZE]) -> Result<Arc<dyn KeyOption>> {
        Self::create_from_expanded_key(ed25519_create_expanded_private_key(exp_key)?)
    }

    /// Create from Ed25519 secret key raw data
    pub fn from_private_key(pvt_key: &[u8; Self::PVT_KEY_SIZE]) -> Result<Arc<dyn KeyOption>> {
        let exp_key = ed25519_expand_private_key(&ed25519_create_private_key(pvt_key)?)?;
        let pub_key = ed25519_create_public_key(&exp_key)?.to_bytes();
        let exp_key = exp_key.to_bytes();
        Ok(Arc::new(Self {
            id: Self::calc_id(Self::KEY_TYPE, &pub_key),
            pub_key: Some(pub_key),
            exp_key: Some(exp_key),
            pvt_key: Some(*pvt_key),
        }))
    }

    /// Create from Ed25519 secret key raw data and export JSON
    pub fn from_private_key_with_json(
        pvt_key: &[u8; Self::PVT_KEY_SIZE],
    ) -> Result<(KeyOptionJson, Arc<dyn KeyOption>)> {
        Self::create_from_private_key_with_json(ed25519_create_private_key(pvt_key)?)
    }

    /// Create from Ed25519 secret key JSON
    pub fn from_private_key_json(src: &KeyOptionJson) -> Result<Arc<dyn KeyOption>> {
        match src.type_id {
            Self::KEY_TYPE => match &src.pvt_key {
                Some(key) => {
                    if src.pub_key.is_some() {
                        fail!("No public key expected");
                    }
                    let key = base64_decode(key)?;
                    if key.len() != Self::PVT_KEY_SIZE {
                        fail!("Bad private key");
                    }
                    Self::from_private_key(key.as_slice().try_into()?)
                }
                None => fail!("No private key"),
            },
            _ => fail!("Type-id {} is not supported for Ed25519 private key", src.type_id),
        }
    }

    /// Create from Ed25519 public key raw data
    pub fn from_public_key(pub_key: &[u8; Self::PUB_KEY_SIZE]) -> Arc<dyn KeyOption> {
        Arc::new(Self {
            id: Self::calc_id(Self::KEY_TYPE, pub_key),
            pub_key: Some(*pub_key),
            exp_key: None,
            pvt_key: None,
        })
    }

    /// Create from Ed265519 public key JSON
    pub fn from_public_key_json(src: &KeyOptionJson) -> Result<Arc<dyn KeyOption>> {
        match src.type_id {
            Self::KEY_TYPE => match &src.pub_key {
                Some(key) => {
                    if src.pvt_key.is_some() {
                        fail!("No private key expected");
                    }
                    let key = base64_decode(key)?;
                    if key.len() != Self::PUB_KEY_SIZE {
                        fail!("Bad public key");
                    }
                    Ok(Self::from_public_key(key.as_slice().try_into()?))
                }
                None => fail!("No public key"),
            },
            _ => fail!("Type-id {} is not supported for Ed25519 public key", src.type_id),
        }
    }

    /// Generate new Ed25519 key
    pub fn generate() -> Result<Arc<dyn KeyOption>> {
        Self::create_from_expanded_key(
            ed25519_expand_private_key(&ed25519_generate_private_key()?)?,
        )
    }

    /// Generate new Ed25519 key and export JSON
    pub fn generate_with_json() -> Result<(KeyOptionJson, Arc<dyn KeyOption>)> {
        Self::create_from_private_key_with_json(ed25519_generate_private_key()?)
    }

    pub fn create_from_expanded_key(
        exp_key: Ed25519ExpandedPrivateKey,
    ) -> Result<Arc<dyn KeyOption>> {
        let pub_key = ed25519_create_public_key(&exp_key)?.to_bytes();
        let exp_key = exp_key.to_bytes();
        let ret = Self {
            id: Self::calc_id(Self::KEY_TYPE, &pub_key),
            pub_key: Some(pub_key),
            exp_key: Some(exp_key),
            pvt_key: None,
        };
        Ok(Arc::new(ret))
    }

    pub fn create_from_private_key_with_json(
        pvt_key: Ed25519PrivateKey,
    ) -> Result<(KeyOptionJson, Arc<dyn KeyOption>)> {
        let ret = Self::create_from_expanded_key(ed25519_expand_private_key(&pvt_key)?)?;
        let json = KeyOptionJson {
            type_id: Self::KEY_TYPE,
            pub_key: None,
            pvt_key: Some(base64_encode(pvt_key.to_bytes())),
        };
        Ok((json, ret))
    }

    // Calculate key ID
    fn calc_id(type_id: i32, pub_key: &[u8; Self::PUB_KEY_SIZE]) -> Arc<KeyId> {
        let data = sha256_digest_slices(&[&type_id.to_le_bytes(), pub_key]);
        KeyId::from_data(data)
    }

    fn exp_key(&self) -> Result<&[u8; Self::EXP_KEY_SIZE]> {
        if let Some(exp_key) = self.exp_key.as_ref() {
            Ok(exp_key)
        } else {
            fail!("No expansion key set for key option {}", self.id())
        }
    }
}

impl KeyOption for Ed25519KeyOption {
    /// Get key id
    fn id(&self) -> &Arc<KeyId> {
        &self.id
    }

    /// Get type id
    fn type_id(&self) -> i32 {
        Self::KEY_TYPE
    }

    /// Get public key
    fn pub_key(&self) -> Result<&[u8]> {
        if let Some(pub_key) = self.pub_key.as_ref() {
            Ok(pub_key)
        } else {
            fail!("No public key set for key option {}", self.id())
        }
    }

    /// Calculate signature
    fn sign(&self, data: &[u8]) -> Result<Vec<u8>> {
        ed25519_sign(self.exp_key()?, self.pub_key().ok(), data)
    }

    /// Verify signature
    fn verify(&self, data: &[u8], signature: &[u8]) -> Result<()> {
        ed25519_verify(self.pub_key()?, data, signature)
    }

    /// Calculate shared secret
    fn shared_secret(&self, other_pub_key: &[u8]) -> Result<[u8; 32]> {
        x25519_shared_secret(self.exp_key()?, other_pub_key)
    }

    #[cfg(feature = "export_key")]
    fn export_key(&self) -> Result<&[u8]> {
        Ok(self.exp_key()?)
    }

    fn pvt_key(&self) -> Result<&[u8; 32]> {
        if let Some(pvt_key) = self.pvt_key.as_ref() {
            Ok(pvt_key)
        } else {
            fail!("No private key set for key option {}", self.id())
        }
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
    type_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pvt_key: Option<String>,
}

impl KeyOptionJson {
    pub fn get_pvt_key(&self) -> Result<Vec<u8>> {
        let Some(key) = &self.pvt_key else {
            fail!("No private key to get from KeyOptionJson");
        };
        base64_decode(key)
    }
    pub fn type_id(&self) -> &i32 {
        &self.type_id
    }
}
