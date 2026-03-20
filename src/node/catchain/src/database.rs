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
use crate::{
    check_execution_time, instrument, utils::MetricsHandle, BlockHash, Database, DatabasePtr,
    RawBuffer,
};
use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use storage::catchain_persistent_db::CatchainPersistentDb;
use ton_block::{fail, Result};

/*
    Implementation details for Database
*/

pub struct DatabaseImpl {
    db: CatchainPersistentDb,         //persistent storage
    put_tx_counter: metrics::Counter, //DB put transactions counter
    get_tx_counter: metrics::Counter, //DB get transactions counter
    destroy_db: Arc<AtomicBool>,      //DB should be destroyed at drop
}

/*
    Implementation for public Database trait
*/

impl Database for DatabaseImpl {
    /*
        Database management
    */

    /// Return path to db
    fn get_db_path(&self) -> &Path {
        self.db.path()
    }

    fn destroy(&self) {
        self.destroy_db.store(true, Ordering::SeqCst);
    }

    /*
        Blocks management
    */

    fn is_block_in_db(&self, hash: &BlockHash) -> bool {
        instrument!();

        self.db.contains(hash).unwrap_or_default()
    }

    fn get_block(&self, hash: &BlockHash) -> Result<RawBuffer> {
        check_execution_time!(50000);
        instrument!();

        self.get_tx_counter.increment(1);

        match self.db.get(hash) {
            Ok(ref data) => Ok(data.as_ref().to_vec()),
            Err(err) => fail!("Block {:x} not found: {:?}", hash, err),
        }
    }

    fn put_block(&self, hash: &BlockHash, data: RawBuffer) {
        check_execution_time!(50000);
        instrument!();

        self.put_tx_counter.increment(1);

        if let Err(err) = self.db.put(hash, &data) {
            log::error!("Block {:x} DB saving error: {:?}", hash, err)
        }
    }

    fn erase_block(&self, hash: &BlockHash) {
        check_execution_time!(50000);
        instrument!();

        if let Err(err) = self.db.delete(hash) {
            log::warn!("Block {:x} DB erasing error: {:?}", hash, err)
        }
    }
}

/*
    Drop implementation for Database
*/

impl Drop for DatabaseImpl {
    fn drop(&mut self) {
        instrument!();

        log::debug!("Dropping Catchain database...");

        if self.destroy_db.load(Ordering::SeqCst) {
            log::debug!("Destroying DB at path '{}'", self.get_db_path().display());
            self.destroy_database();
        }

        log::debug!("Catchain database has been successfully dropped");
    }
}

/*
    Private DatabaseImpl details
*/

impl DatabaseImpl {
    fn destroy_database(&mut self) {
        if let Err(err) = self.db.destroy() {
            log::error!("cannot destroy catchain db: {}", err)
        }
    }

    pub(crate) fn create(
        path: &str,
        name: &str,
        metrics_receiver: &MetricsHandle,
    ) -> Result<DatabasePtr> {
        log::debug!("Creating catchain table in DB at path '{}'", path);

        let put_tx_counter = metrics_receiver.sink().register_counter(&"db_put_txs".into());
        let get_tx_counter = metrics_receiver.sink().register_counter(&"db_get_txs".into());
        let db = CatchainPersistentDb::new(path, name)?;

        let ret = Self {
            db,
            put_tx_counter,
            get_tx_counter,
            destroy_db: Arc::new(AtomicBool::new(false)),
        };
        Ok(Arc::new(ret))
    }
}
