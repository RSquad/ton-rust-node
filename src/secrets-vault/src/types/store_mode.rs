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
pub enum StoreMode {
    /// Store only if the entry does not already exist
    NewOnly,

    /// Replace the entry only if it already exists
    ReplaceExists,

    /// Always store, creating or replacing the entry unconditionally
    CreateOrReplace,
}

impl Display for StoreMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            StoreMode::NewOnly => "NewOnly",
            StoreMode::ReplaceExists => "ReplaceExists",
            StoreMode::CreateOrReplace => "CreateOrReplace",
        };
        f.write_str(s)
    }
}
