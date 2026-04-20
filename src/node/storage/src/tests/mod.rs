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
mod test_catchain_persistent_db;
mod test_dynamic_boc_archive_db;
mod test_dynamic_boc_rc_db;
mod test_shardstate_db_async;

pub mod utils {

    #[cfg(feature = "telemetry")]
    use crate::StorageTelemetry;
    use crate::{
        block_handle_db::{BlockHandleDb, BlockHandleStorage, NodeStateDb},
        db::rocksdb::RocksDb,
        StorageAlloc,
    };
    use fnv::FnvHashSet;
    use std::sync::Arc;
    use ton_block::{read_single_root_boc, BlockIdExt, Cell, ShardIdent, UInt256, SHARD_FULL};

    include!("../../../../common/src/test.rs");

    pub fn get_test_shard_ident() -> ShardIdent {
        ShardIdent::with_tagged_prefix(-1, SHARD_FULL).unwrap()
    }

    pub static ROOT_HASH: [u8; 32] = [
        0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE,
        0xFF, 0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54,
        0x32, 0x10,
    ];

    pub static FILE_HASH: [u8; 32] = [
        0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32,
        0x10, 0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD,
        0xEE, 0xFF,
    ];

    pub fn count_tree_unique_cells(root_cell: Cell) -> usize {
        let mut unique_cells = FnvHashSet::default();
        count_tree_unique_cells_recursive(root_cell, &mut unique_cells);
        unique_cells.len()
    }

    fn count_tree_unique_cells_recursive(cell: Cell, unique_cells: &mut FnvHashSet<UInt256>) {
        if unique_cells.insert(cell.repr_hash()) {
            for i in 0..cell.references_count() {
                count_tree_unique_cells_recursive(cell.reference(i).unwrap(), unique_cells);
            }
        }
    }

    pub fn create_block_handle_storage(
        db: Arc<RocksDb>,
    ) -> (BlockHandleStorage, Arc<BlockHandleDb>) {
        let block_handle_db =
            Arc::new(BlockHandleDb::with_db(db.clone(), "block_handles", true).unwrap());
        let block_handle_storage = BlockHandleStorage::with_dbs(
            block_handle_db.clone(),
            Arc::new(NodeStateDb::with_db(db.clone(), "full_node_states", true).unwrap()),
            Arc::new(NodeStateDb::with_db(db, "validator_states", true).unwrap()),
            #[cfg(feature = "telemetry")]
            Arc::new(StorageTelemetry::default()),
            Arc::new(StorageAlloc::default()),
        );
        (block_handle_storage, block_handle_db)
    }

    pub fn get_test_block_id() -> BlockIdExt {
        // -1,8000000000000000,1830539
        BlockIdExt::with_params(
            get_test_shard_ident(),
            1830539,
            UInt256::from(&ROOT_HASH),
            UInt256::from(&FILE_HASH),
        )
    }

    pub fn get_test_tree_of_cells() -> Cell {
        let data = include_bytes!("testdata/2467080").to_vec();
        read_single_root_boc(data).unwrap()
    }

    pub fn get_another_test_tree_of_cells() -> Cell {
        let data = include_bytes!("testdata/2467119").to_vec();
        read_single_root_boc(data).unwrap()
    }
}
