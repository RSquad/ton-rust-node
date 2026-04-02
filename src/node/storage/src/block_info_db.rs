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
use crate::db_impl_base;
use ton_block::BlockIdExt;

pub const PREV1_BLOCK_DB_NAME: &str = "prev1_block_db";
pub const PREV2_BLOCK_DB_NAME: &str = "prev2_block_db";
pub const NEXT1_BLOCK_DB_NAME: &str = "next1_block_db";
pub const NEXT2_BLOCK_DB_NAME: &str = "next2_block_db";

db_impl_base!(BlockInfoDb, BlockIdExt);
