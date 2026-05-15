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
    memory::protected_memory::ProtectedMemoryInner, utils::hex::hex_decode,
};
use std::ffi::{CStr, CString};

pub struct MasterKey {
    key_material: KeyMaterial,
}

impl MasterKey {
    pub fn from_env(var_name: &str) -> anyhow::Result<Self> {
        let key_material = Self::load_from_env(var_name)?;
        Ok(Self { key_material })
    }

    pub fn from_key_material(key_material: KeyMaterial) -> anyhow::Result<Self> {
        Ok(Self { key_material })
    }

    pub fn key_material(&self) -> &KeyMaterial {
        &self.key_material
    }

    pub fn try_clone(&self) -> anyhow::Result<Self> {
        Ok(MasterKey { key_material: self.key_material.try_clone()? })
    }

    fn getenv_nocopy(name: &CStr) -> Option<&CStr> {
        unsafe {
            let ptr = libc::getenv(name.as_ptr());

            if ptr.is_null() {
                None
            } else {
                Some(CStr::from_ptr(ptr))
            }
        }
    }

    fn load_from_env(var_name: &str) -> anyhow::Result<KeyMaterial> {
        let var_name_c_string = CString::new(var_name)?;
        let key_hex_ptr = match Self::getenv_nocopy(var_name_c_string.as_c_str()) {
            Some(ptr) => ptr,
            None => {
                anyhow::bail!(VaultError::master_key_unavailable(format!(
                    "ENV variable '{}' not found",
                    var_name
                )))
            }
        };

        let key_hex = key_hex_ptr.to_bytes();

        if key_hex.len() != 64 {
            anyhow::bail!(VaultError::master_key_unavailable(format!(
                "Wrong ENV variable value for '{}'. Key must be a 64-character hexadecimal string",
                var_name,
            )));
        }

        let mut key = ProtectedMemoryInner::new(key_hex.len() / 2)?;
        {
            let mut handle = key.write_handle()?;
            hex_decode(key_hex, handle.as_mut()).map_err(|e| {
                VaultError::master_key_unavailable(format!(
                    "Wrong ENV variable '{}' value. {}",
                    var_name, e
                ))
            })?;
        }

        KeyMaterial::new_symmetric_key(key.into())
    }
}
