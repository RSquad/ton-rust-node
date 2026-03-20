/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::{
    error::Error,
    fmt::{Display, Formatter},
    str::FromStr,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum TonWalletVersion {
    V1R3,
    V3R2,
    V4R2,
    V5R1,
}

impl Display for TonWalletVersion {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            TonWalletVersion::V1R3 => write!(f, "V1R3"),
            TonWalletVersion::V3R2 => write!(f, "V3R2"),
            TonWalletVersion::V4R2 => write!(f, "V4R2"),
            TonWalletVersion::V5R1 => write!(f, "V5R1"),
        }
    }
}

#[derive(Debug)]
pub struct ParseTonWalletVersionError;

impl Display for ParseTonWalletVersionError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseTonWalletVersionError => write!(f, "Failed to parse TON wallet version"),
        }
    }
}

impl FromStr for TonWalletVersion {
    type Err = ParseTonWalletVersionError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "V1R3" => Ok(TonWalletVersion::V1R3),
            "V3R2" => Ok(TonWalletVersion::V3R2),
            "V4R2" => Ok(TonWalletVersion::V4R2),
            "V5R1" => Ok(TonWalletVersion::V5R1),
            _ => Err(ParseTonWalletVersionError),
        }
    }
}

impl Error for ParseTonWalletVersionError {}

pub mod version_serde {
    use super::TonWalletVersion;
    use serde::Deserialize;
    use std::str::FromStr;

    pub fn serialize<S>(
        version: &TonWalletVersion,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let str = version.to_string();
        serializer.serialize_str(str.as_str())
    }

    pub fn deserialize<'de, D>(deserializer: D) -> std::result::Result<TonWalletVersion, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        TonWalletVersion::from_str(s.as_str()).map_err(serde::de::Error::custom)
    }
}
