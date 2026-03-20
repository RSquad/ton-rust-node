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
use ton_block::BlockIdExt;

#[derive(Debug, PartialEq, thiserror::Error)]
pub enum StorageError {
    /// Key not found
    #[error("Key not found: {0}({1})")]
    KeyNotFound(&'static str, String),

    /// Reading out of buffer range
    #[error("Reading out of buffer range")]
    OutOfRange,

    #[error("Attempt to load state {0} which is already allowed to GC")]
    StateIsAllowedToGc(BlockIdExt),
}
