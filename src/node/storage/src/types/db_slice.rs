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
use rocksdb::DBPinnableSlice;
use std::ops::Deref;

/// Represents memory slice, returned by database (in a case of RocksDB), or vector, in a case of MemoryDb
pub enum DbSlice<'a> {
    RocksDbTable(DBPinnableSlice<'a>),
    Vector(Vec<u8>),
    Slice(&'a [u8]),
}

impl AsRef<[u8]> for DbSlice<'_> {
    fn as_ref(&self) -> &[u8] {
        match self {
            DbSlice::RocksDbTable(slice) => slice.as_ref(),
            DbSlice::Vector(vector) => vector.as_slice(),
            DbSlice::Slice(slice) => slice,
        }
    }
}

impl Deref for DbSlice<'_> {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        self.as_ref()
    }
}

impl<'a> From<DBPinnableSlice<'a>> for DbSlice<'a> {
    fn from(slice: DBPinnableSlice<'a>) -> Self {
        DbSlice::RocksDbTable(slice)
    }
}

impl<'a> From<&'a [u8]> for DbSlice<'a> {
    fn from(slice: &'a [u8]) -> Self {
        DbSlice::Slice(slice)
    }
}

impl<'a> From<Vec<u8>> for DbSlice<'a> {
    fn from(vector: Vec<u8>) -> Self {
        DbSlice::Vector(vector)
    }
}
