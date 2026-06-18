/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use crate::{
    crypto::crypto_trait::Crypto,
    errors::error::VaultError,
    memory::protected_memory::{ProtectedMemory, ProtectedMemoryInner},
    types::algorithm::Algorithm,
};

pub struct KeyMaterial {
    pub secret_key: Option<ProtectedMemory>,
    pub public_key: Option<Vec<u8>>,
    pub tags: Option<Vec<String>>,
}

impl KeyMaterial {
    pub const TAG_EXPANDED_KEY: &str = "expanded";

    pub fn new(
        secret_key: Option<ProtectedMemory>,
        public_key: Option<Vec<u8>>,
    ) -> anyhow::Result<Self> {
        if secret_key.is_none() && public_key.is_none() {
            anyhow::bail!(VaultError::invalid_key_size("Secret and public keys are empty"));
        }

        if let Some(s) = secret_key.as_ref() {
            if s.is_empty() {
                anyhow::bail!(VaultError::empty_secret_key("Failed to create key material"));
            }
        }

        if let Some(p) = public_key.as_ref() {
            if p.is_empty() {
                anyhow::bail!(VaultError::empty_public_key("Failed to create key material"));
            }
        }

        Ok(Self { secret_key, public_key, tags: None })
    }

    pub fn new_blob(data: ProtectedMemory) -> anyhow::Result<Self> {
        Ok(Self { secret_key: Some(data), public_key: None, tags: None })
    }

    pub fn new_pvt_pub(private_key: ProtectedMemory, public_key: Vec<u8>) -> anyhow::Result<Self> {
        if private_key.is_empty() {
            anyhow::bail!(VaultError::empty_secret_key("Private key is empty"));
        }

        if public_key.is_empty() {
            anyhow::bail!(VaultError::empty_public_key("Public key is empty"));
        }

        Ok(Self { secret_key: Some(private_key), public_key: Some(public_key), tags: None })
    }

    pub fn new_exp_pub(expanded_key: ProtectedMemory, public_key: Vec<u8>) -> anyhow::Result<Self> {
        if expanded_key.is_empty() {
            anyhow::bail!(VaultError::empty_secret_key("Expanded key is empty"));
        }

        if public_key.is_empty() {
            anyhow::bail!(VaultError::empty_public_key("Public key is empty"));
        }

        Ok(Self {
            secret_key: Some(expanded_key),
            public_key: Some(public_key),
            tags: Some(vec![Self::TAG_EXPANDED_KEY.to_string()]),
        })
    }

    pub fn new_pub_key(public_key: Vec<u8>) -> anyhow::Result<Self> {
        if public_key.is_empty() {
            anyhow::bail!(VaultError::empty_public_key("Public key is empty"));
        }

        Ok(Self { secret_key: None, public_key: Some(public_key), tags: None })
    }

    pub fn new_symmetric_key(key: ProtectedMemory) -> anyhow::Result<Self> {
        if key.is_empty() {
            anyhow::bail!(VaultError::empty_secret_key("Symmetric key is empty"));
        }

        Ok(Self { secret_key: Some(key), public_key: None, tags: None })
    }

    pub fn generate_new(
        algorithm: Algorithm,
        size: Option<usize>,
        crypto: &dyn Crypto,
    ) -> anyhow::Result<Self> {
        let key_material = match algorithm {
            Algorithm::None => {
                let blob = crypto.generate_rnd(algorithm, size)?;
                Self::new_blob(blob)?
            }
            Algorithm::Aes256Gcm => {
                let key = crypto.generate_rnd(algorithm, size)?;
                Self::new_symmetric_key(key)?
            }
            Algorithm::Ed25519 => {
                let private_key = crypto.generate_rnd(algorithm, size)?;
                let public_key = crypto.pub_key_from_pvt(algorithm, &private_key.lock()?)?;
                Self::new_pvt_pub(private_key, public_key)?
            }
        };

        Ok(key_material)
    }

    pub fn try_clone(&self) -> anyhow::Result<Self> {
        let secret_key = match self.secret_key.as_ref() {
            Some(pm) => Some(pm.try_clone()?),
            None => None,
        };

        Ok(KeyMaterial { secret_key, public_key: self.public_key.clone(), tags: self.tags.clone() })
    }

    pub fn serialize(&self) -> anyhow::Result<ProtectedMemory> {
        if self.tags.as_ref().is_some_and(|t| !t.is_empty()) {
            anyhow::bail!(VaultError::invalid_key_material(
                "Tags are only supported at runtime and can't be serialized"
            ))
        }

        // Format: [secret_key_len: u32][public_key_len: u32][secret_key][public_key]
        let secret_key_guard = match &self.secret_key {
            Some(secret_key) => Some(secret_key.lock()?),
            None => None,
        };

        let secret_key_len = secret_key_guard.as_ref().map_or(0, |guard| guard.len());
        let public_key_len = self.public_key.as_ref().map_or(0, |pk| pk.len());

        let mut serialized = ProtectedMemoryInner::new(0)?;
        {
            let mut handle = serialized.write_handle()?;

            handle.extend_from_slice(&(secret_key_len as u32).to_le_bytes())?;
            handle.extend_from_slice(&(public_key_len as u32).to_le_bytes())?;

            if let Some(ref guard) = secret_key_guard {
                handle.extend_from_slice(guard)?;
            }

            if let Some(ref public_key) = self.public_key {
                handle.extend_from_slice(public_key)?;
            }
        }

        Ok(serialized.into())
    }

    pub fn deserialize(source_data: &[u8]) -> anyhow::Result<Self> {
        // Format: [secret_key_len: u32][public_key_len: u32][secret_key][public_key]
        if source_data.len() < 8 {
            anyhow::bail!(VaultError::invalid_key_material(
                "invalid source_data: too short to contain lengths"
            ));
        }

        let secret_key_len = u32::from_le_bytes(source_data[0..4].try_into().unwrap()) as usize;
        let public_key_len = u32::from_le_bytes(source_data[4..8].try_into().unwrap()) as usize;

        if (secret_key_len == 0) && (public_key_len == 0) {
            anyhow::bail!(VaultError::invalid_key_material(
                "Secret key length and public key length are 0"
            ));
        }

        let total_needed = 8usize
            .checked_add(secret_key_len)
            .and_then(|v| v.checked_add(public_key_len))
            .ok_or_else(|| VaultError::invalid_key_material("invalid lengths: overflow"))?;

        if source_data.len() != total_needed {
            anyhow::bail!(VaultError::invalid_key_material(format!(
                "Invalid source_data length: expected {}, got {}",
                total_needed,
                source_data.len()
            )));
        }

        let mut offset = 4 + 4;

        let secret_key = if secret_key_len > 0 {
            let secret_key = &source_data[offset..offset + secret_key_len];
            offset += secret_key_len;

            Some(ProtectedMemoryInner::from_slice(secret_key)?.into())
        } else {
            None
        };

        let public_key = if public_key_len > 0 {
            Some(source_data[offset..offset + public_key_len].to_vec())
        } else {
            None
        };

        Self::new(secret_key, public_key)
    }

    pub fn eq_km(&self, other: &KeyMaterial) -> anyhow::Result<bool> {
        let secret_keys_equal = match (&self.secret_key, &other.secret_key) {
            (Some(a), Some(b)) => a.eq_pm(b)?,
            (None, None) => true,
            _ => false,
        };
        Ok(self.public_key == other.public_key && secret_keys_equal)
    }
}
