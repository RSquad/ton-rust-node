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
pub mod test_filedb;
pub mod test_rocksdb;

pub mod utils {

    use crate::{db::DbKey, error::StorageError};
    use ton_block::Result;

    pub fn expect_key_not_found_error<T, K: DbKey>(result: Result<T>, key: K) {
        match result {
            Ok(_) => panic!("We don't expect any value to return"),
            Err(error) => {
                let kind =
                    error.downcast::<StorageError>().expect("Expected error of type StorageError");
                match kind {
                    StorageError::KeyNotFound(_key_name, err_key) => {
                        assert!(err_key.starts_with(key.as_string().as_str()))
                    }
                    _ => panic!("Expected KeyNotFound error"),
                }
            }
        }
    }

    pub fn expect_error<T>(result: Result<T>, expected_error: StorageError) {
        match result {
            Ok(_) => panic!("We don't expect any value to return"),
            Err(error) => {
                let kind =
                    error.downcast::<StorageError>().expect("Expected error of type StorageError");
                if kind != expected_error {
                    panic!("Expected {} error", expected_error)
                }
            }
        }
    }
}

impl super::DbKey for &[u8] {
    fn key_name(&self) -> &'static str {
        "&[u8]"
    }

    fn key(&self) -> &[u8] {
        self
    }
}
