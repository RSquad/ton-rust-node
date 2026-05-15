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
use std::sync::Arc;
use ton_block::{
    calc_d1, calc_d2, cell_type, fail, full_len, level_mask, refs_count, store_hashes, Cell,
    CellType, Result, UInt256, DEPTH_SIZE, MAX_HASHES_COUNT, MAX_LEVEL, MAX_REFERENCES_COUNT,
    SHA256_SIZE,
};

// Max raw data: d1(1) + d2(1) + hashes(32*4) + depths(2*4) + data(128) + ref_hashes(32*4) + ref_depths(2*4)
pub const STORED_CELL_MAX_RAW_LEN: usize = 1 + 1 + 32 * 4 + 2 * 4 + 128 + 32 * 4 + 2 * 4;

#[cfg(test)]
#[path = "tests/test_storage_cell.rs"]
mod tests;

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

/// Deserialize cell data from DB and return a `Cell` (LoadedCell variant).
pub fn deserialize_stored_cell(
    repr_hash: &UInt256,
    data: &[u8],
    loader: &Arc<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>,
) -> Result<Cell> {
    // Note: Cell::check_data is NOT called here to avoid double validation.
    // with_data_and_loader will validate the reconstructed buffer.
    if data.len() < 2 {
        fail!("Buffer is too small to read description bytes");
    }

    let has_store_hashes = store_hashes(data);
    let cell_full_len = full_len(data);
    let cell_type = cell_type(data);
    let ref_count = refs_count(data);
    let mut reader = SliceReader::new(&data[cell_full_len..]);

    let mut raw = smallvec::SmallVec::<[u8; STORED_CELL_MAX_RAW_LEN]>::new();
    if has_store_hashes {
        // Some cell stored by older versions of node may have all hashes stored in cell data,
        // so we just read it.
        // Current version always stores hashes separately (see else branch)
        raw.extend_from_slice(&data[..cell_full_len]);
    } else {
        // Build standard cell layout (with stored hashes set)
        // [d1 + store_hashes] [d2] [hashes] [depths BE] [cell_data]
        // from stored cell layout + given repr hash
        // [d1] [d2] [cell_data] [[high hash, [high depth LE]] [repr depth LE]

        let level_mask = level_mask(data);
        let d1_new = calc_d1(level_mask, true, cell_type, ref_count);
        let d2 = data[1];

        // Collect hashes and depths
        // For cells with level > 0, high hashes are stored after cell data
        let mut hashes = smallvec::SmallVec::<[&[u8]; MAX_HASHES_COUNT]>::new();
        let mut depths = smallvec::SmallVec::<[u16; MAX_HASHES_COUNT]>::new();

        let lvl = level_mask.level() as usize;
        if cell_type != CellType::PrunedBranch && lvl > 0 {
            for _ in 0..lvl {
                let hash = reader.read(SHA256_SIZE)?;
                let depth_bytes: [u8; 2] = reader.read(DEPTH_SIZE)?.try_into()?;
                let depth = u16::from_le_bytes(depth_bytes);
                hashes.push(hash);
                depths.push(depth);
            }
        }

        // Repr hash/depth — repr hash is the DB key, depth is stored separately
        let repr_depth_bytes: [u8; 2] = reader.read(DEPTH_SIZE)?.try_into()?;
        let repr_depth = u16::from_le_bytes(repr_depth_bytes);
        hashes.push(repr_hash.as_slice());
        depths.push(repr_depth);

        // Build raw: [d1_new] [d2] [hashes BE] [depths BE] [cell_data]
        let cell_data = &data[2..cell_full_len]; // cell data without d1/d2
        raw.push(d1_new);
        raw.push(d2);
        for h in &hashes {
            raw.extend_from_slice(h);
        }
        for &d in &depths {
            raw.extend_from_slice(&d.to_be_bytes());
        }
        raw.extend_from_slice(cell_data);
    }

    // Read references (child repr hash + child repr depth)
    let mut ref_hashes = smallvec::SmallVec::<[UInt256; MAX_REFERENCES_COUNT]>::new();
    let mut ref_depths = smallvec::SmallVec::<[u16; MAX_REFERENCES_COUNT]>::new();
    for _ in 0..ref_count {
        let hash = UInt256::from_slice(reader.read(SHA256_SIZE)?);
        let depth_bytes: [u8; 2] = reader.read(DEPTH_SIZE)?.try_into()?;
        let depth = u16::from_le_bytes(depth_bytes);
        ref_hashes.push(hash);
        ref_depths.push(depth);
    }

    let total_read = cell_full_len + reader.position;
    if total_read != data.len() {
        fail!(
            "There is more data after storage cell deserialisation (read: {}, data len: {})",
            total_read,
            data.len()
        );
    }

    Cell::with_data_and_loader(&raw, false, &ref_hashes, &ref_depths, loader, None)
}

pub fn serialize_stored_cell(
    cell: &Cell,
) -> Result<smallvec::SmallVec<[u8; STORED_CELL_MAX_RAW_LEN]>> {
    let mut data = smallvec::SmallVec::new();

    // While deserialization we have cell's repr hash, because it is the key in DB,
    // so we don't need to store it in cell data. This way we need to implement custom
    // serialization logic for stored cells, which is different from regular BOC serialization.
    //
    // Stored cell layout:
    // [d1] [d2] [cell_data] [[high_hash, high_depth LE]...] [repr_depth LE]
    // [[ref_hash, ref_depth LE]...]

    // d1 without store_hashes flag + d2 + cell data
    let level_mask = cell.level_mask();
    let d1 = calc_d1(level_mask, false, cell.cell_type(), cell.references_count());
    data.push(d1);
    data.push(calc_d2(cell.bit_length())); // d2
    data.extend_from_slice(cell.data()); // cell data bytes

    // High hashes and depths (pruned branches store them inside cell data)
    if cell.cell_type() != CellType::PrunedBranch {
        let mask = level_mask.mask();
        for i in 0..MAX_LEVEL {
            if (1 << i) & mask != 0 {
                data.extend_from_slice(cell.hash(i).as_slice());
                data.extend_from_slice(&cell.depth(i).to_le_bytes());
            }
        }
    }

    // Repr depth (repr hash is the DB key, not stored)
    data.extend_from_slice(&cell.depth(MAX_LEVEL).to_le_bytes());

    // References (child repr hash + child repr depth)
    for i in 0..cell.references_count() {
        let h = cell.reference_repr_hash(i)?;
        let d = cell.reference_repr_depth(i)?;
        data.extend_from_slice(h.as_slice());
        data.extend_from_slice(&d.to_le_bytes());
    }

    Ok(data)
}
