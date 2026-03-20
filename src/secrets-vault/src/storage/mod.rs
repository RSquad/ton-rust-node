/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "file-storage-json")]
pub mod file_json;

#[cfg(feature = "file-storage-json")]
pub(crate) mod file_json_migrator;

#[cfg(feature = "hashicorp-storage")]
pub mod hashicorp;

#[cfg(feature = "hashicorp-storage")]
pub(crate) mod hashicorp_api;

pub mod storage_trait;
pub mod utils;
