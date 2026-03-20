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
use strum_macros::AsRefStr;

#[derive(Debug, AsRefStr)]
pub enum StatusKey {
    // TODO: Reserved for DynamicBocDb
}

impl DbKey for StatusKey {
    fn key_name(&self) -> &'static str {
        "StatusKey"
    }

    fn as_string(&self) -> String {
        self.as_ref().to_string()
    }
    fn key(&self) -> &[u8] {
        self.as_ref().as_bytes()
    }
}
