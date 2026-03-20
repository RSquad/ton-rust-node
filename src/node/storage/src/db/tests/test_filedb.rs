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
    db::{
        filedb::FileDb,
        tests::utils::{expect_error, expect_key_not_found_error},
    },
    error::StorageError,
};
use std::{
    io::Write,
    ops::{Deref, DerefMut},
    path::Path,
};
use ton_block::Result;

const KEY0: &[u8] = b"key0key0key0key0";
const KEY1: &[u8] = b"key1key1key1key1";

struct AutoDestroyableDb {
    db: FileDb,
}

impl AutoDestroyableDb {
    pub fn new(path: &str) -> Self {
        Self { db: FileDb::with_path(path) }
    }
}

impl Deref for AutoDestroyableDb {
    type Target = FileDb;

    fn deref(&self) -> &Self::Target {
        &self.db
    }
}

impl DerefMut for AutoDestroyableDb {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.db
    }
}

impl Drop for AutoDestroyableDb {
    fn drop(&mut self) {
        if self.path().is_dir() {
            std::fs::remove_dir_all(self.db.path()).expect("Failed to destroy DB");
        }
    }
}

#[test]
fn test_make_path() {
    let db = FileDb::with_path("/test");
    let key = vec![
        0x12, 0x34, 0x56, 0x78, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0xFF, 0xEE, 0x87, 0x65, 0x43,
        0x21,
    ];
    let path = db.make_path(key.as_slice(), false);
    assert_eq!(path, Path::new("/test/1234/5678aabbccddeeffffee87654321"));

    let path = db.make_path(key.as_slice(), true);
    assert_eq!(path, Path::new("/test/1234/5678aabbccddeeffffee87654321.tmp"));
}

#[tokio::test]
async fn test_get() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_get");

    expect_key_not_found_error(db.read_whole_file(&KEY0).await, KEY0);
    db.write_whole_file(&KEY0, &[0]).await?;
    assert_eq!(&db.read_whole_file(&KEY0).await?, &[0]);
    expect_key_not_found_error(db.read_whole_file(&KEY1).await, KEY1);

    Ok(())
}

#[tokio::test]
async fn test_put() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_put");

    db.write_whole_file(&KEY0, &[0]).await?;
    assert_eq!(&db.read_whole_file(&KEY0).await?, &[0]);
    expect_key_not_found_error(db.read_whole_file(&KEY1).await, KEY1);
    db.write_whole_file(&KEY1, &[1]).await?;
    assert_eq!(&db.read_whole_file(&KEY1).await?, &[1]);

    Ok(())
}

#[tokio::test]
async fn test_delete() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_delete");

    db.delete_file(&KEY0).await?;

    expect_key_not_found_error(db.read_whole_file(&KEY0).await, KEY0);
    db.write_whole_file(&KEY0, &[0]).await?;
    assert_eq!(&db.read_whole_file(&KEY0).await?, &[0]);
    expect_key_not_found_error(db.read_whole_file(&KEY1).await, KEY1);
    db.write_whole_file(&KEY1, &[1]).await?;
    assert_eq!(&db.read_whole_file(&KEY1).await?, &[1]);

    db.delete_file(&KEY0).await?;
    expect_key_not_found_error(db.read_whole_file(&KEY0).await, KEY0);
    assert_eq!(&db.read_whole_file(&KEY1).await?, &[1]);
    db.delete_file(&KEY1).await?;
    expect_key_not_found_error(db.read_whole_file(&KEY1).await, KEY1);

    Ok(())
}

#[tokio::test]
async fn test_destroy() -> Result<()> {
    let mut db = AutoDestroyableDb::new("filedb_test_destroy");
    assert!(!db.path().is_dir());

    db.destroy().await?;

    db.write_whole_file(&KEY0, &[0]).await?;
    assert!(db.path().is_dir());

    db.destroy().await?;

    assert!(!db.path().is_dir());

    Ok(())
}

#[tokio::test]
async fn test_get_size() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_get_size");

    expect_key_not_found_error(db.get_file_size(&KEY0).await, KEY0);

    db.write_whole_file(&KEY0, &[0, 1, 2, 3]).await?;
    assert_eq!(db.get_file_size(&KEY0).await?, 4);

    db.write_whole_file(&KEY1, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]).await?;
    assert_eq!(db.get_file_size(&KEY1).await?, 10);

    db.write_whole_file(&KEY0, &[]).await?;
    assert_eq!(db.get_file_size(&KEY0).await?, 0);

    Ok(())
}

#[tokio::test]
async fn test_contains() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_contains");

    assert!(!db.contains(&KEY0).await?);

    db.write_whole_file(&KEY0, &[0]).await?;
    assert!(db.contains(&KEY0).await?);

    assert!(!db.contains(&KEY1).await?);

    db.write_whole_file(&KEY1, &[1]).await?;
    assert!(db.contains(&KEY1).await?);

    Ok(())
}

#[tokio::test]
async fn test_get_slice() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_get_slice");

    expect_key_not_found_error(db.read_file_part(&KEY0, 0, 5).await, KEY0);

    db.write_whole_file(&KEY0, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]).await?;
    assert_eq!(&db.read_file_part(&KEY0, 0, 1).await?, &[0]);
    assert_eq!(&db.read_file_part(&KEY0, 0, 10).await?, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(&db.read_file_part(&KEY0, 5, 5).await?, &[5, 6, 7, 8, 9]);
    assert_eq!(&db.read_file_part(&KEY0, 5, 2).await?, &[5, 6]);

    expect_error(db.read_file_part(&KEY0, 7, 5).await, StorageError::OutOfRange);
    expect_error(db.read_file_part(&KEY0, 7, 4).await, StorageError::OutOfRange);
    expect_error(db.read_file_part(&KEY0, 11, 1).await, StorageError::OutOfRange);

    Ok(())
}

#[tokio::test]
async fn test_for_each_key() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_for_each_key");

    for i in 1..=100 {
        let key = vec![i; 32];
        let val = vec![i; 1024];
        db.write_whole_file(&key, &val).await?;
    }

    let mut sum = 0_u32;

    db.for_each_key(&mut |key: &[u8]| {
        sum += key[0] as u32;
        Ok(true)
    })?;

    assert_eq!(sum, 5050);

    Ok(())
}

#[tokio::test]
async fn test_read_whole_file_to() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_read_whole_file_to");

    db.write_whole_file(&KEY0, &[0, 1, 2, 3]).await?;

    let mut buf = Vec::new();
    db.read_whole_file_to(&KEY0, &mut buf).await?;
    assert_eq!(buf, vec![0, 1, 2, 3]);

    let mut buf = vec![0, 0, 0, 0, 0];
    buf.truncate(0);
    assert_eq!(buf.capacity(), 5);
    db.read_whole_file_to(&KEY0, &mut buf).await?;
    assert_eq!(buf, vec![0, 1, 2, 3]);

    Ok(())
}

#[tokio::test]
async fn test_get_write_object() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_get_write_object");

    let mut file = db.get_write_object(&KEY0)?;
    file.write(&[0, 1, 2, 3, 4])?;
    drop(file);
    assert!(db.read_whole_file(&KEY0).await.is_err());
    db.finalize_write_object(&KEY0)?;
    assert_eq!(vec![0, 1, 2, 3, 4], db.read_whole_file(&KEY0).await?);

    let mut file = db.get_write_object(&KEY0)?;
    file.write(&[4, 5, 6, 7])?;
    drop(file);
    db.finalize_write_object(&KEY0)?;
    assert_eq!(vec![4, 5, 6, 7], db.read_whole_file(&KEY0).await?);

    Ok(())
}

#[tokio::test]
async fn test_cleanup_tmp() -> Result<()> {
    let db = AutoDestroyableDb::new("filedb_test_cleanup_tmp");

    let mut file1 = db.get_write_object(&KEY0)?;
    file1.write(&[0, 1, 2, 3, 4])?;
    drop(file1);

    let mut file2 = db.get_write_object(&KEY1)?;
    file2.write(&[5, 6, 7, 8, 9])?;
    drop(file2);
    db.finalize_write_object(&KEY1)?;

    let path = db.make_path(&KEY0, true);
    assert!(path.exists());

    let path = db.make_path(&KEY1, true);
    assert!(!path.exists());
    let path = db.make_path(&KEY1, false);
    assert!(path.exists());

    db.cleanup_tmp()?;

    let path = db.make_path(&KEY0, true);
    assert!(!path.exists());
    let path = db.make_path(&KEY0, false);
    assert!(!path.exists());

    let path = db.make_path(&KEY1, true);
    assert!(!path.exists());
    let path = db.make_path(&KEY1, false);
    assert!(path.exists());

    Ok(())
}
