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
use crate::db::DbKey;
use ton_block::BlockIdExt;

impl DbKey for BlockIdExt {
    fn key_name(&self) -> &'static str {
        "BlockId"
    }
    fn as_string(&self) -> String {
        format!("{}", self)
    }
    fn key(&self) -> &[u8] {
        self.root_hash().as_slice()
    }
}
