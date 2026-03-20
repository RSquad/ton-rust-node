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
#[macro_export]
macro_rules! db_impl_base {
    ($type: ident, $key_type: ty) => {
        pub struct $type {
            db: $crate::db::rocksdb::RocksDbTable<$key_type>,
        }

        impl $type {
            /// Constructs new instance using RocksDB with given path
            pub fn with_db(
                db: std::sync::Arc<$crate::db::rocksdb::RocksDb>,
                family: impl ToString,
                create_if_not_exist: bool,
            ) -> ton_block::Result<Self> {
                Ok(Self { db: db.table(family, create_if_not_exist)? })
            }
        }

        impl std::ops::Deref for $type {
            type Target = $crate::db::rocksdb::RocksDbTable<$key_type>;
            fn deref(&self) -> &Self::Target {
                &self.db
            }
        }

        impl std::ops::DerefMut for $type {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.db
            }
        }
    };
}

#[macro_export]
macro_rules! db_impl_single {
    ($type: ident) => {
        pub struct $type {
            db: std::sync::Arc<$crate::db::rocksdb::RocksDb>,
        }

        impl $type {
            /// Constructs new instance using RocksDB with given path
            pub fn new(path: impl AsRef<std::path::Path>, name: &str) -> ton_block::Result<Self> {
                Ok(Self {
                    db: $crate::db::rocksdb::RocksDb::new(
                        path,
                        name,
                        None,
                        $crate::db::rocksdb::AccessType::ReadWrite,
                    )?,
                })
            }

            /// Destroys instance of RocksDB with given path
            pub fn destroy(&mut self) -> ton_block::Result<bool> {
                let path = self.db.path().to_path_buf();
                if let Some(db) = std::sync::Arc::get_mut(&mut self.db) {
                    if let Err(err) = db.destroy() {
                        ton_block::fail!("Database {:?} destroying error: {}", path, err)
                    } else {
                        Ok(true)
                    }
                } else {
                    ton_block::fail!("operation pending in db {:?}", path)
                }
            }

            pub fn path(&self) -> &std::path::Path {
                self.db.path()
            }

            pub fn db(&self) -> &std::sync::Arc<$crate::db::rocksdb::RocksDb> {
                &self.db
            }
        }

        impl std::ops::Deref for $type {
            type Target = RocksDb;

            fn deref(&self) -> &Self::Target {
                self.db.deref()
            }
        }
    };
}

#[macro_export]
macro_rules! db_impl_cbor {
    ($type: ident, $key_type: ty, $value_type: ty) => {
        $crate::db_impl_base!($type, $key_type);

        impl $type {
            #[allow(dead_code)]
            pub fn value(
                &self,
                db_slice: impl AsRef<[u8]>,
            ) -> std::result::Result<$value_type, serde_cbor::Error> {
                serde_cbor::from_slice(db_slice.as_ref())
            }

            #[allow(dead_code)]
            pub fn try_get_value(&self, key: &$key_type) -> ton_block::Result<Option<$value_type>> {
                if let Some(db_slice) = self.try_get(key)? {
                    return Ok(Some(serde_cbor::from_slice(db_slice.as_ref())?));
                }
                Ok(None)
            }

            #[allow(dead_code)]
            pub fn get_value(&self, key: &$key_type) -> ton_block::Result<$value_type> {
                Ok(serde_cbor::from_slice(self.get(key)?.as_ref())?)
            }

            pub fn put_value(&self, key: &$key_type, value: &$value_type) -> ton_block::Result<()> {
                self.put(key, &serde_cbor::to_vec(value)?)
            }
        }
    };
}

#[macro_export]
macro_rules! db_impl_serializable {
    ($type: ident, $key_type: ty) => {
        $crate::db_impl_base!($type, $key_type);

        impl $type {
            pub fn try_get_value<T: Serializable>(
                &self,
                key: &$key_type,
            ) -> ton_block::Result<Option<T>> {
                if let Some(db_slice) = self.db.try_get(key)? {
                    Ok(Some(T::deserialize(db_slice.as_ref())?))
                } else {
                    Ok(None)
                }
            }

            pub fn get_value<T: Serializable>(&self, key: &$key_type) -> ton_block::Result<T> {
                T::deserialize(self.db.get(key)?.as_ref())
            }

            pub fn put_value<T: Serializable>(
                &self,
                key: &$key_type,
                value: &T,
            ) -> ton_block::Result<()> {
                self.db.put(key, value.serialize().as_ref())
            }
        }
    };
}
