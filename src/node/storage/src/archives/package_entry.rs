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
use crate::{traits::Serializable, TARGET};
use ton_block::{fail, Result};

pub(crate) const PKG_ENTRY_HEADER_SIZE: usize = 8;
const PKG_ENTRY_HEADER_MAGIC: u16 = 0x1E8B;

#[derive(Debug)]
struct PackageEntryHeader {
    filename_size: u16,
    data_size: u32,
}

impl Serializable for PackageEntryHeader {
    const SIZE: usize = PKG_ENTRY_HEADER_SIZE;
    type Bytes = [u8; Self::SIZE];
    fn serialize(&self) -> Self::Bytes {
        let mut ret = [0u8; Self::SIZE];
        ret[..2].copy_from_slice(&PKG_ENTRY_HEADER_MAGIC.serialize());
        ret[2..4].copy_from_slice(&self.filename_size.serialize());
        ret[4..].copy_from_slice(&self.data_size.serialize());
        ret
    }
    fn deserialize_checked(data: &[u8]) -> Result<Self> {
        let magic = u16::deserialize(data)?;
        if magic != PKG_ENTRY_HEADER_MAGIC {
            fail!("Bad entry magic: 0x{magic:X}")
        }
        let filename_size = u16::deserialize(&data[2..])?;
        let data_size = u32::deserialize(&data[4..])?;
        let ret = Self { filename_size, data_size };
        Ok(ret)
    }
}

pub struct PackageEntry {
    filename: String,
    data: Vec<u8>,
}

impl PackageEntry {
    pub const fn with_data(filename: String, data: Vec<u8>) -> Self {
        Self { filename, data }
    }

    pub(super) async fn read_from<R: tokio::io::AsyncReadExt + Unpin>(
        reader: &mut R,
    ) -> Result<Option<Self>> {
        let mut buf = [0; PKG_ENTRY_HEADER_SIZE];
        match reader.read_exact(&mut buf).await {
            Ok(count) => assert_eq!(count, buf.len()),
            Err(error) => {
                return if error.kind() == tokio::io::ErrorKind::UnexpectedEof {
                    Ok(None)
                } else {
                    Err(error.into())
                }
            }
        }
        let entry_header = PackageEntryHeader::deserialize(&buf)?;
        let mut buf = vec![0; entry_header.filename_size as usize];
        reader.read_exact(&mut buf).await?;
        let filename = String::from_utf8(buf)?;
        log::trace!(
            target: TARGET,
            "Reading package entry: {filename}, size: {}",
            entry_header.data_size
        );
        let mut data = vec![0; entry_header.data_size as usize];
        reader.read_exact(&mut data).await?;
        Ok(Some(Self::with_data(filename, data)))
    }

    pub(super) async fn write_to<W: tokio::io::AsyncWriteExt + Unpin>(
        &self,
        writer: &mut W,
    ) -> Result<u64> {
        let entry_header = PackageEntryHeader {
            filename_size: self.filename.len() as u16,
            data_size: self.data.len() as u32,
        };
        writer.write_all(&entry_header.serialize()).await?;
        writer.write_all(self.filename.as_bytes()).await?;
        writer.write_all(&self.data).await?;
        writer.flush().await?;
        let size = PKG_ENTRY_HEADER_SIZE as u64
            + entry_header.filename_size as u64
            + entry_header.data_size as u64;
        Ok(size)
    }

    pub fn filename(&self) -> &str {
        self.filename.as_str()
    }

    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }

    pub fn take_data(self) -> Vec<u8> {
        self.data
    }
}
