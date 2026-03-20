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
use crate::catchain_persistent_db::CatchainPersistentDb;
use ton_block::Result;

#[test]
fn test_destroy() -> Result<()> {
    let mut db = CatchainPersistentDb::new("../../target/test", "catchains")?;
    db.destroy()?;

    Ok(())
}
