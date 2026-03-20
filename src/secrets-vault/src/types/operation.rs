/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use std::fmt::{Display, Formatter};

#[derive(Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Operation {
    Sign,
    VerifySign,
    Encrypt,
    Decrypt,
    Extract,
}

impl Display for Operation {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Operation::Sign => "Sign",
            Operation::VerifySign => "VerifySign",
            Operation::Encrypt => "Encrypt",
            Operation::Decrypt => "Decrypt",
            Operation::Extract => "Extract",
        };
        f.write_str(s)
    }
}
