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
    archives::package_entry::{PackageEntry, PKG_ENTRY_HEADER_SIZE},
    TARGET,
};
use std::{
    io::SeekFrom,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use ton_block::{error, fail, Result};

#[derive(Debug)]
pub struct Package {
    path: PathBuf,
    read_only: bool,
    size: AtomicU64,
    write_mutex: tokio::sync::Mutex<Option<tokio::fs::File>>,
}

pub(crate) const PKG_HEADER_SIZE: usize = 4;
const PKG_HEADER_MAGIC: u32 = 0xAE8F_DD01;

async fn read_header<R: tokio::io::AsyncReadExt + Unpin>(reader: &mut R) -> Result<()> {
    let mut buf = [0; PKG_HEADER_SIZE];
    if reader.read_exact(&mut buf).await? != PKG_HEADER_SIZE {
        fail!("Package file read failed")
    }
    if u32::from_le_bytes(buf) != PKG_HEADER_MAGIC {
        fail!("Package file header mismatch")
    }

    Ok(())
}

impl Package {
    pub async fn open(path: PathBuf, read_only: bool, create: bool) -> Result<Self> {
        let mut file = Self::open_file_ext(read_only, create, path.as_path()).await?;
        let mut size = file.metadata().await?.len();

        file.seek(SeekFrom::Start(0)).await?;
        if size < PKG_HEADER_SIZE as u64 {
            if !create {
                fail!("Package file is too short")
            }
            file.write_all(&PKG_HEADER_MAGIC.to_le_bytes()).await?;
            file.flush().await?;
            size = PKG_HEADER_SIZE as u64;
        } else {
            read_header(&mut file).await?;
            file.seek(SeekFrom::End(0)).await?;
        }

        Ok(Self {
            path,
            read_only,
            size: AtomicU64::new(size),
            write_mutex: tokio::sync::Mutex::new(Some(file)),
        })
    }

    pub fn size(&self) -> u64 {
        self.size.load(Ordering::SeqCst) - PKG_HEADER_SIZE as u64
    }

    pub async fn remove(&self) -> Result<()> {
        debug_assert!(!self.read_only);
        let mut file = self.write_mutex.lock().await;
        if file.take().is_none() {
            Ok(())
        } else {
            Self::remove_by_path(self.path()).await
        }
    }

    pub async fn remove_by_path(path: &Path) -> Result<()> {
        if path.try_exists()? {
            tokio::fs::remove_file(path)
                .await
                .map_err(|err| error!("destroy package error {}", err))?;
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    pub fn get_path(&self) -> String {
        self.path.display().to_string()
    }

    pub async fn truncate(&self, size: u64) -> Result<()> {
        let new_size = PKG_HEADER_SIZE as u64 + size;
        // let md = tokio::fs::metadata(self.path()).await?;
        // if md.len() == new_size {
        //     return Ok(())
        // }
        log::debug!(
            target: TARGET,
            "Truncating package {}, new size: {new_size} bytes",
            self.path.display()
        );
        self.size.store(new_size, Ordering::SeqCst);
        let Some(file) = &*self.write_mutex.lock().await else {
            fail!(
                "Cannot truncate package file {}, because it was not opened",
                self.path().display()
            )
        };
        file.set_len(new_size).await?;
        Ok(())
    }

    pub async fn read_entry(&self, offset: u64) -> Result<PackageEntry> {
        let mut file = self.open_file().await?;
        let required_size = offset + PKG_ENTRY_HEADER_SIZE as u64;
        // Fast path: use cached atomic size for bounds check.
        let mut actual_size = self.size();
        if actual_size <= required_size {
            // Slow path: cached size may be stale when another thread just finished
            // appending; confirm with actual file metadata before returning EOF.
            actual_size = file.metadata().await?.len().saturating_sub(PKG_HEADER_SIZE as u64);
            if actual_size <= required_size {
                fail!(
                    "Unexpected end of file when reading package {} entry with offset {offset}",
                    self.path.display()
                );
            } else {
                log::warn!(
                    target: TARGET,
                    "Not actualized package {} size {actual_size} \
                    when reading entry with offset {offset}",
                    self.path.display()
                )
            }
        }
        file.seek(SeekFrom::Start(PKG_HEADER_SIZE as u64 + offset)).await?;
        PackageEntry::read_from(&mut file)
            .await?
            .ok_or_else(|| error!("Package::read_entry: Unexpected end of file"))
    }

    pub async fn append_entry(
        &self,
        entry: &PackageEntry,
        after_append: impl FnOnce(u64, u64) -> Result<()>,
    ) -> Result<()> {
        debug_assert!(entry.filename().len() <= u16::MAX as usize);
        debug_assert!(entry.data().len() <= u32::MAX as usize);
        let Some(file) = &mut *self.write_mutex.lock().await else {
            fail!(
                "Cannot append entry to package file {}, because it was not opened",
                self.path().display()
            )
        };
        let actual = file.metadata().await?.len();
        let entry_offset = self.size();
        if entry_offset + PKG_HEADER_SIZE as u64 != actual {
            log::error!(
                target: TARGET,
                "Package entry {} offset mismatch: expected {entry_offset} vs {actual}",
                entry.filename()
            )
        }
        let entry_size = entry.write_to(file).await?;
        let total_size = self.size.fetch_add(entry_size, Ordering::SeqCst) + entry_size;
        let actual = file.metadata().await?.len();
        if total_size != actual {
            log::error!(
                target: TARGET,
                "Package entry {} size mismatch: expected {total_size} vs {actual}",
                entry.filename()
            )
        }
        after_append(entry_offset, entry_offset + entry_size)
    }

    pub async fn open_file(&self) -> Result<tokio::fs::File> {
        Self::open_file_ext(self.read_only, false, self.path.as_path()).await
    }

    async fn open_file_ext(
        read_only: bool,
        create: bool,
        path: impl AsRef<Path>,
    ) -> Result<tokio::fs::File> {
        Ok(tokio::fs::OpenOptions::new()
            .read(true)
            .write(!read_only || create)
            .create(create)
            .open(&path)
            .await?)
    }
}

pub struct PackageReader<R: tokio::io::AsyncReadExt + Unpin> {
    reader: tokio::io::BufReader<R>,
}

impl<R: tokio::io::AsyncReadExt + Unpin> PackageReader<R> {
    pub async fn next(&mut self) -> Result<Option<PackageEntry>> {
        PackageEntry::read_from(&mut self.reader).await
    }
}

pub async fn read_package_from_file(
    path: impl AsRef<Path>,
) -> Result<PackageReader<tokio::fs::File>> {
    read_package_from(
        tokio::fs::OpenOptions::new().read(true).write(false).create(false).open(path).await?,
    )
    .await
}

pub async fn read_package_from<R: tokio::io::AsyncReadExt + Unpin>(
    reader: R,
) -> Result<PackageReader<R>> {
    let mut reader = tokio::io::BufReader::with_capacity(1 << 19, reader);
    read_header(&mut reader).await?;
    Ok(PackageReader::<R> { reader })
}

#[cfg(test)]
#[path = "../tests/test_package.rs"]
mod tests;
