/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::types::payload::PayloadType;
use std::{
    fmt::{Display, Formatter},
    str::FromStr,
};

#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[non_exhaustive]
pub enum Algorithm {
    #[default]
    None,
    Aes256Gcm,
    Ed25519,
}

impl Algorithm {
    pub fn payload_type(&self) -> PayloadType {
        match self {
            Self::None => PayloadType::Blob,
            Self::Aes256Gcm => PayloadType::SymmetricKey,
            Self::Ed25519 => PayloadType::KeyPair,
        }
    }

    pub fn can_sign(&self) -> bool {
        match self {
            Self::None => false,
            Self::Aes256Gcm => false,
            Self::Ed25519 => true,
        }
    }

    pub fn can_encrypt(&self) -> bool {
        match self {
            Self::None => false,
            Self::Aes256Gcm => true,
            Self::Ed25519 => false,
        }
    }

    pub fn is_blob(&self) -> bool {
        self.payload_type() == PayloadType::Blob
    }

    pub fn is_symmetric(&self) -> bool {
        self.payload_type() == PayloadType::SymmetricKey
    }

    pub fn is_asymmetric(&self) -> bool {
        self.payload_type() == PayloadType::KeyPair
    }

    pub fn key_bits(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Aes256Gcm => 256,
            Self::Ed25519 => 256,
        }
    }

    pub fn all_supported() -> Vec<Algorithm> {
        let algos = vec![Algorithm::None, Algorithm::Aes256Gcm, Algorithm::Ed25519];

        algos
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Algorithm::None => "None",
            Algorithm::Aes256Gcm => "Aes256Gcm",
            Algorithm::Ed25519 => "Ed25519",
        }
    }
}

impl Display for Algorithm {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Algorithm::None => write!(f, "None"),
            Algorithm::Aes256Gcm => write!(f, "AES-256-GCM"),
            Algorithm::Ed25519 => write!(f, "Ed25519"),
        }
    }
}

impl FromStr for Algorithm {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "none" => Ok(Algorithm::None),
            "aes256gcm" | "aes-256-gcm" => Ok(Algorithm::Aes256Gcm),
            "ed25519" => Ok(Algorithm::Ed25519),
            _ => anyhow::bail!("Unknown algorithm name '{}'", s),
        }
    }
}
