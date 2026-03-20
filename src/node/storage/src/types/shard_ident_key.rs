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
use crate::{db::DbKey, traits::Serializable};
use ton_block::{Result, ShardIdent};

pub struct ShardIdentKey([u8; ShardIdent::SIZE]);

impl ShardIdentKey {
    pub fn new(shard_ident: &ShardIdent) -> Result<Self> {
        Ok(Self(shard_ident.serialize()))
    }
}

impl DbKey for ShardIdentKey {
    fn key_name(&self) -> &'static str {
        "ShardIdentKey"
    }

    fn as_string(&self) -> String {
        ShardIdent::deserialize(self.key())
            .map(|shard_ident| shard_ident.to_string())
            .unwrap_or_else(|_err| hex::encode(self.key()))
    }

    fn key(&self) -> &[u8] {
        let Self(key) = self;
        key.as_slice()
    }
}
