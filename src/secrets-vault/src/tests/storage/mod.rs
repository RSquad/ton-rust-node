/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
#[cfg(feature = "file-storage-json")]
mod file_json_migration;
#[cfg(feature = "file-storage-json")]
mod file_storage_json;

#[cfg(feature = "hashicorp-storage")]
mod hashicorp_api;

pub(crate) mod storage_trait;
