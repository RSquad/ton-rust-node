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
use crate::block_handle_db::BlockHandle;

mod package_index_db;

pub mod archive_manager;
pub mod db_provider;
pub mod epoch;
pub mod package;
pub mod package_entry;
pub mod package_entry_id;

mod archive_slice;
mod block_index_db;
mod file_maps;
mod package_entry_meta_db;
pub mod package_id;
mod package_info;
mod package_offsets_db;
mod package_status_db;
mod package_status_key;

pub const ARCHIVE_SIZE: u32 = 100_000;
pub const ARCHIVE_SLICE_SIZE: u32 = 20_000;
pub const ARCHIVE_PACKAGE_SIZE: u32 = 100;

pub const KEY_ARCHIVE_SIZE: u32 = 10_000_000;
pub const KEY_ARCHIVE_SLICE_SIZE: u32 = 2_000_000;
pub const KEY_ARCHIVE_PACKAGE_SIZE: u32 = 200_000;

fn get_mc_seq_no(handle: &BlockHandle) -> u32 {
    if handle.id().shard().is_masterchain() {
        handle.id().seq_no()
    } else {
        handle.masterchain_ref_seq_no()
    }
}
