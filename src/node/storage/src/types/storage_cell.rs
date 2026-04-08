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
use crate::{dynamic_boc_rc_db::DynamicBocDb, TARGET};
use std::{
    io::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Weak,
    },
};
use ton_block::{
    calc_d1, cell_type, error, fail, full_len, hashes_count, level, level_mask, refs_count,
    store_hashes, Cell, CellData, CellImpl, CellType, LevelMask, Result, UInt256, DEPTH_SIZE,
    MAX_LEVEL, SHA256_SIZE,
};

#[cfg(test)]
#[path = "tests/test_storage_cell.rs"]
mod tests;

const NOT_INITIALIZED_DEPTH: u16 = u16::MAX;

struct Reference {
    hash: UInt256,
    depth: u16,
    cell: Option<Weak<dyn CellImpl>>,
}

pub struct StoredCell {
    cell_data: CellData,
    references: parking_lot::RwLock<Vec<Reference>>,
    boc_db: Weak<DynamicBocDb>,
}

static STORED_CELL_COUNT: AtomicU64 = AtomicU64::new(0);

struct SliceReader<'a> {
    data: &'a [u8],
    position: usize,
}
impl<'a> SliceReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }
    fn read(&mut self, size: usize) -> Result<&'a [u8]> {
        if self.data.len() < self.position + size {
            fail!("Buffer is too small to read {} bytes", size);
        }
        let slice = &self.data[self.position..self.position + size];
        self.position += size;
        Ok(slice)
    }
}

/// Represents Cell for storing in persistent storage
impl StoredCell {
    pub fn deserialize(
        boc_db: &Arc<DynamicBocDb>,
        repr_hash: &UInt256,
        data: &[u8],
    ) -> Result<Self> {
        if data.len() < 2 {
            fail!("Buffer is too small to read description bytes");
        }

        // Cell data (same as in BOC)
        let mut cell_data = CellData::with_unbounded_raw_data_slice(data)?;
        let mut reader = SliceReader::new(&data[cell_data.raw_data().len()..]);

        // If the cell data isn't contain stored high hashes - read it now
        let level = cell_data.level();
        let store_hashes = cell_data.store_hashes();
        let mut hash_array_index = 0;
        if level > 0 && // there are high hashes
           cell_data.cell_type() != CellType::PrunedBranch && // pruned branche stores high hashes in the data
           !store_hashes
        // some cells store high hashes in the raw data
        {
            for _ in 0..level {
                let hash = reader.read(32)?;
                let depth = u16::from_le_bytes(reader.read(2)?.try_into()?);
                cell_data.set_hash_depth(hash_array_index, hash, depth)?;
                hash_array_index += 1;
            }
        }

        if !store_hashes {
            // Representation depth without hash, because DB key is repr hash
            let depth = u16::from_le_bytes(reader.read(2)?.try_into()?);
            cell_data.set_hash_depth(hash_array_index, repr_hash.as_slice(), depth)?;
        }

        // References (child repr hash + child repr depth)
        let references_count = cell_data.references_count();
        let mut references = Vec::with_capacity(references_count);
        for _ in 0..references_count {
            let hash = UInt256::from_slice(reader.read(32)?);
            let depth = u16::from_le_bytes(reader.read(2)?.try_into()?);
            references.push(Reference { hash, depth, cell: None });
        }

        let read = cell_data.raw_data().len() + reader.position;
        // need to remove check 16 in future
        if read != data.len() && read + 16 != data.len() {
            fail!(
                "There is more data after storage cell deserialisation (read: {}, data len: {})",
                read,
                data.len()
            );
        }

        STORED_CELL_COUNT.fetch_add(1, Ordering::Relaxed);
        boc_db.allocated().storage_cells.fetch_add(1, Ordering::Relaxed);

        Ok(Self {
            cell_data,
            references: parking_lot::RwLock::new(references),
            boc_db: Arc::downgrade(boc_db),
        })
    }

    pub fn write_cell_data(
        data: &[u8],
        repr_hash: &UInt256,
        write_hashes: bool,
        dest: &mut dyn Write,
    ) -> Result<()> {
        // Cell layout:
        // [D1] [D2] (hashes: 0..4 big endian u256) (depths: 0..4 big endian u16) [data: 0..128 bytes]

        // Storage cell data (stores hashes if cell itself doesn't):
        // [Cell layout] ((hash, depth), ...) [(child_repr_hash, depth), ...]

        let has_hashes = store_hashes(data);
        let full_len = full_len(data);

        match (has_hashes, write_hashes) {
            (true, true) | (false, false) => {
                dest.write_all(&data[..full_len])?;
            }
            (true, false) => {
                let d1 = calc_d1(level_mask(data), false, cell_type(data), refs_count(data));
                dest.write_all(&[d1])?;
                dest.write_all(&data[1..2])?; // D2
                let hashes_len = (SHA256_SIZE + DEPTH_SIZE) * hashes_count(data);
                dest.write_all(&data[2 + hashes_len..full_len])?; // data
            }
            (false, true) => {
                // repack with hashes

                let d1 = calc_d1(level_mask(data), true, cell_type(data), refs_count(data));
                dest.write_all(&[d1])?;
                dest.write_all(&data[1..2])?; // D2

                // hashes
                let level = level(data) as usize;
                for i in 0..level {
                    let offset = if cell_type(data) == CellType::PrunedBranch {
                        2 + 1 + 1 + i * SHA256_SIZE
                    } else {
                        full_len + i * (SHA256_SIZE + DEPTH_SIZE)
                    };
                    dest.write_all(&data[offset..offset + SHA256_SIZE])?;
                }
                dest.write_all(repr_hash.as_slice())?;

                // depths
                for i in 0..level {
                    let offset = if cell_type(data) == CellType::PrunedBranch {
                        2 + 1 + 1 + level * SHA256_SIZE + i * DEPTH_SIZE
                    } else {
                        full_len + i * (SHA256_SIZE + DEPTH_SIZE) + SHA256_SIZE
                    };
                    // depths are stored in little-endian in cells db, but in big-endian in BOC format
                    dest.write_all(&[data[offset + 1], data[offset]])?;
                }
                if cell_type(data) == CellType::PrunedBranch {
                    dest.write_all(&[0, 0])?;
                } else {
                    // repr depth is stored without hash
                    let offset = full_len + level * (SHA256_SIZE + DEPTH_SIZE);
                    // depths are stored in little-endian in cells db, but in big-endian in BOC format
                    dest.write_all(&[data[offset + 1], data[offset]])?;
                }

                dest.write_all(&data[2..full_len])?; // data
            }
        }

        Ok(())
    }

    pub fn cell_count() -> u64 {
        STORED_CELL_COUNT.load(Ordering::Relaxed)
    }

    pub fn calc_serialized_size(
        raw_data_len: usize,
        store_hashes: bool,
        level: u8,
        references_count: usize,
        cell_type: CellType,
    ) -> usize {
        let mut data_size = raw_data_len; // data
        if !store_hashes {
            if cell_type != CellType::PrunedBranch {
                data_size += level as usize * 34; // hash + depth
            }
            data_size += 2; // repr depth
        }
        data_size += references_count * 34; // reference hash + depth
        data_size
    }

    pub fn serialize(cell: &dyn CellImpl) -> Result<Vec<u8>> {
        let store_hashes = cell.store_hashes();
        let data_size = Self::calc_serialized_size(
            cell.raw_data()?.len(),
            store_hashes,
            cell.level(),
            cell.references_count(),
            cell.cell_type(),
        );

        let mut data = Vec::with_capacity(data_size);
        data.extend_from_slice(cell.raw_data()?);

        if !store_hashes {
            if cell.cell_type() != CellType::PrunedBranch {
                let level_mask = cell.level_mask().mask();
                if level_mask != 0 {
                    for i in 0..MAX_LEVEL {
                        if (1 << i) & level_mask != 0 {
                            data.extend_from_slice(cell.hash(i).as_slice());
                            data.extend_from_slice(&cell.depth(i).to_le_bytes());
                        }
                    }
                }
            }
            data.extend_from_slice(&cell.depth(MAX_LEVEL).to_le_bytes());
        }

        for i in 0..cell.references_count() {
            data.extend_from_slice(cell.reference_repr_hash(i)?.as_slice());
            data.extend_from_slice(&cell.reference_repr_depth(i)?.to_le_bytes());
        }
        Ok(data)
    }

    pub fn with_cell_data(
        cell_data: CellData,
        refs: &[(UInt256, u16)],
        boc_db: &Arc<DynamicBocDb>,
    ) -> Result<Self> {
        if cell_data.references_count() != refs.len() {
            fail!("References count mismatch: {} != {}", cell_data.references_count(), refs.len());
        }
        if cell_data.level() != 0 {
            fail!("Cell data must have zero level");
        }
        let mut references = Vec::with_capacity(refs.len());
        for (hash, depth) in refs {
            references.push(Reference { hash: hash.clone(), depth: *depth, cell: None });
        }
        STORED_CELL_COUNT.fetch_add(1, Ordering::Relaxed);
        boc_db.allocated().storage_cells.fetch_add(1, Ordering::Relaxed);
        Ok(Self {
            cell_data,
            references: parking_lot::RwLock::new(references),
            boc_db: Arc::downgrade(boc_db),
        })
    }
}

impl Drop for StoredCell {
    fn drop(&mut self) {
        STORED_CELL_COUNT.fetch_sub(1, Ordering::Relaxed);
        if let Some(boc_db) = self.boc_db.upgrade() {
            boc_db.allocated().storage_cells.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl PartialEq for StoredCell {
    fn eq(&self, other: &Self) -> bool {
        self.cell_data.raw_hash(MAX_LEVEL) == other.cell_data.raw_hash(MAX_LEVEL)
    }
}

pub struct StoringCell {
    cell_data: CellData,
    references: parking_lot::RwLock<Vec<Reference>>,
    boc_db: Weak<DynamicBocDb>,
}

impl PartialEq for StoringCell {
    fn eq(&self, other: &Self) -> bool {
        self.cell_data.raw_hash(MAX_LEVEL) == other.cell_data.raw_hash(MAX_LEVEL)
    }
}

impl StoringCell {
    pub fn with_cell(cell: &dyn CellImpl, boc_db: &Arc<DynamicBocDb>) -> Result<Self> {
        let references_count = cell.references_count();
        let mut references = Vec::with_capacity(references_count);
        for i in 0..references_count {
            let hash = cell.reference_repr_hash(i)?;
            let depth = cell.reference_repr_depth(i)?;
            log::trace!(target: TARGET, "Cell {:x} - reference [{}] {:x} is taken", 
                cell.hash(MAX_LEVEL), i, hash);
            references.push(Reference {
                hash,
                depth,
                cell: Some(Arc::downgrade(cell.reference(i)?.cell_impl())),
            });
        }
        let mut cell_data = CellData::with_raw_data(cell.raw_data()?.to_vec())?;
        if !cell.store_hashes() {
            let mut hash_index = 0;
            let level_mask = cell.level_mask().mask();
            if level_mask != 0 && cell.cell_type() != CellType::PrunedBranch {
                for i in 0..MAX_LEVEL {
                    if (1 << i) & level_mask != 0 {
                        cell_data.set_hash_depth(
                            hash_index,
                            cell.hash(i).as_slice(),
                            cell.depth(i),
                        )?;
                        hash_index += 1;
                    }
                }
            }
            cell_data.set_hash_depth(
                hash_index,
                cell.hash(MAX_LEVEL).as_slice(),
                cell.depth(MAX_LEVEL),
            )?;
        }
        Ok(Self {
            cell_data,
            references: parking_lot::RwLock::new(references),
            boc_db: Arc::downgrade(boc_db),
        })
    }
}

macro_rules! define_CellImpl {
    ( $type_name:ident ) => {
        impl CellImpl for $type_name {
            fn data(&self) -> &[u8] {
                self.cell_data.data()
            }

            fn raw_data(&self) -> Result<&[u8]> {
                Ok(self.cell_data.raw_data())
            }

            fn bit_length(&self) -> usize {
                self.cell_data.bit_length()
            }

            fn references_count(&self) -> usize {
                self.cell_data.references_count()
            }

            fn reference(&self, index: usize) -> Result<Cell> {
                Ok(Cell::with_cell_impl_arc(reference(
                    index,
                    &self.references,
                    &self.boc_db,
                    &|| self.hash(MAX_LEVEL),
                )?))
            }

            fn reference_repr_hash(&self, index: usize) -> Result<UInt256> {
                Ok(self
                    .references
                    .read()
                    .get(index)
                    .ok_or_else(|| error!("There is no reference #{}", index))?
                    .hash
                    .clone())
            }

            fn reference_repr_depth(&self, index: usize) -> Result<u16> {
                let guard = self.references.read();
                let r =
                    guard.get(index).ok_or_else(|| error!("There is no reference #{}", index))?;

                if r.depth != NOT_INITIALIZED_DEPTH {
                    Ok(r.depth)
                } else {
                    drop(guard);
                    let cell =
                        reference(index, &self.references, &self.boc_db, &|| self.hash(MAX_LEVEL))?;
                    let depth = cell.depth(MAX_LEVEL);
                    self.references.write()[index].depth = depth;
                    Ok(depth)
                }
            }

            fn cell_type(&self) -> CellType {
                self.cell_data.cell_type()
            }

            fn level_mask(&self) -> LevelMask {
                self.cell_data.level_mask()
            }

            fn hash(&self, index: usize) -> UInt256 {
                self.cell_data.hash(index)
            }

            fn depth(&self, index: usize) -> u16 {
                self.cell_data.depth(index)
            }

            fn store_hashes(&self) -> bool {
                self.cell_data.store_hashes()
            }
        }
    };
}

define_CellImpl!(StoredCell);
define_CellImpl!(StoringCell);

fn reference(
    index: usize,
    references: &parking_lot::RwLock<Vec<Reference>>,
    boc_db: &Weak<DynamicBocDb>,
    repr_hash: &dyn Fn() -> UInt256,
) -> Result<Arc<dyn CellImpl>> {
    let hash = {
        let references = references.read();
        let reference =
            references.get(index).ok_or_else(|| error!("Reference #{index} not found"))?;
        if let Some(weak) = &reference.cell {
            if let Some(cell) = weak.upgrade() {
                return Ok(cell);
            } else {
                log::trace!(target: TARGET, "Cell {:x} - reference [{}] {:x} was freed", 
                    repr_hash(), index, reference.hash);
            }
        } else {
            log::trace!(target: TARGET, "Cell {:x} - reference [{}] {:x} is None", 
                repr_hash(), index, reference.hash);
        }
        reference.hash.clone()
    };

    let boc_db = boc_db.upgrade().ok_or_else(|| error!("BocDb is dropped"))?;
    let cell = boc_db.load_cell(&hash, true)?;

    references.write()[index].cell = Some(Arc::downgrade(cell.cell_impl()) as Weak<dyn CellImpl>);

    Ok(cell.cell_impl().clone())
}
