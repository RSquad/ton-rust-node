/*
 * Copyright (C) 2019-2024 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
pub mod filedb;
pub mod rocksdb;

#[cfg(test)]
mod tests;

use ton_block::UInt256;

/// Trait for database key
pub trait DbKey {
    fn key_name(&self) -> &'static str;

    fn as_string(&self) -> String {
        hex::encode(self.key())
    }

    fn key(&self) -> &[u8];
}

impl DbKey for &str {
    fn key_name(&self) -> &'static str {
        "&str"
    }

    fn as_string(&self) -> String {
        String::from_utf8_lossy(self.key()).to_string()
    }

    fn key(&self) -> &[u8] {
        self.as_bytes()
    }
}

impl DbKey for UInt256 {
    fn key_name(&self) -> &'static str {
        "UInt256"
    }

    fn key(&self) -> &[u8] {
        self.as_slice()
    }
}

#[derive(Debug)]
pub struct U32Key {
    key: [u8; 4],
}

impl U32Key {
    pub fn with_value(value: u32) -> Self {
        Self { key: value.to_le_bytes() }
    }
}

impl From<u32> for U32Key {
    fn from(value: u32) -> Self {
        Self::with_value(value)
    }
}

impl DbKey for U32Key {
    fn key_name(&self) -> &'static str {
        "U32Key"
    }

    fn as_string(&self) -> String {
        u32::from_le_bytes(self.key).to_string()
    }

    fn key(&self) -> &[u8] {
        &self.key
    }
}

impl DbKey for Vec<u8> {
    fn key_name(&self) -> &'static str {
        "&[u8]"
    }

    fn key(&self) -> &[u8] {
        self
    }
}

impl DbKey for String {
    fn key_name(&self) -> &'static str {
        "String"
    }

    fn as_string(&self) -> String {
        self.clone()
    }

    fn key(&self) -> &[u8] {
        self.as_bytes()
    }
}
