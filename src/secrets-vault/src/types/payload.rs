/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default)]
pub enum PayloadType {
    SymmetricKey, // Symmetric key
    KeyPair,      // Asymmetric key pair
    #[default]
    Blob, // Any data
}

impl Display for PayloadType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PayloadType::SymmetricKey => "SymmetricKey",
            PayloadType::KeyPair => "KeyPair",
            PayloadType::Blob => "Blob",
        };
        f.write_str(s)
    }
}
