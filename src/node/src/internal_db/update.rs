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
use crate::internal_db::{InternalDb, CURRENT_DB_VERSION, DB_VERSION_7, DB_VERSION_8};
use std::sync::atomic::AtomicBool;
use ton_block::{fail, Result};

pub async fn update(
    db: InternalDb,
    mut version: u32,
    _check_stop: &(dyn Fn() -> Result<()> + Sync),
    _is_broken: Option<&AtomicBool>,
    _force_check_db: bool,
    _restore_db_enabled: bool,
) -> Result<InternalDb> {
    if version == DB_VERSION_7 {
        // No updates needed, just update version
        version = DB_VERSION_8;
        db.store_db_version(version)?;
        log::info!(
            "Database updated to version {}. Older versions of the node can't open it anymore.",
            version
        );
    }

    if version != CURRENT_DB_VERSION {
        fail!("Wrong database version {}, supported: {}", version, CURRENT_DB_VERSION);
    }

    Ok(db)
}
