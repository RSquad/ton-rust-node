/*
 * Copyright (C) 2019-2023 EverX. All Rights Reserved.
 * Modifications Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This file has been modified from its original version.
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    bits_to_bytes,
    cell::{
        append_tag, find_tag, Cell, CellType, DataCell, LevelMask, SliceData, MAX_DATA_BITS,
        MAX_SAFE_DEPTH,
    },
    fail, ExceptionCode, Result,
};
use std::{
    convert::From,
    fmt::{Binary, Display, Formatter, UpperHex},
};

const EXACT_CAPACITY: usize = 128;
pub(crate) type SmallData = smallvec::SmallVec<[u8; EXACT_CAPACITY]>;

trait AppendBits {
    fn extend(&mut self, slice: &[u8]) -> &mut [u8];
    fn length_in_bits(&self) -> usize;
    fn truncate(&mut self, len: usize);
    fn update_length_in_bits(&mut self, bits: usize);

    fn append_raw_data(
        &mut self,
        slice: &[u8],
        mut bits: usize,
        capacity: Option<usize>,
    ) -> Result<()> {
        if bits != 0 {
            if slice.len() * 8 < bits {
                fail!(ExceptionCode::FatalError)
            }
            let dst_len = self.length_in_bits();
            if let Some(capacity) = capacity {
                if dst_len + bits > capacity {
                    fail!(ExceptionCode::CellOverflow)
                }
            }
            // Append algorithm:
            // If current length is not a multiple of 8 (last byte is not full), then
            // - copy slice.len() - 1 bytes into buffer and get ref to buffer starting from originally last byte
            // - iterate over ref, combine adjacent bytes one by one and store them in-place
            // - store last byte of slice
            // If current length is a multiple of 8 (last byte is full), then
            // - just push slice into
            let full_bytes = bits / 8;
            let shift = dst_len % 8;
            if shift > 0 {
                self.truncate(1 + dst_len / 8);
                bits -= full_bytes * 8;
                let last_shift = shift + bits;
                let dst = if last_shift <= 8 {
                    self.push(&slice[..full_bytes], full_bytes * 8 + bits)
                } else {
                    self.push(&slice[..full_bytes], full_bytes * 8)
                };
                let mut x = (dst[0] as u16) >> (8 - shift);
                for i in 0..full_bytes {
                    x = (x << 8) | (dst[i + 1] as u16);
                    dst[i] = (x >> shift) as u8;
                }
                x <<= 8;
                if bits > 0 {
                    x |= slice[full_bytes] as u16
                }
                dst[full_bytes] = (x >> shift) as u8;
                if last_shift != 8 {
                    let mask = 0xFFu8 << (8 - last_shift % 8);
                    if last_shift < 8 {
                        dst[full_bytes] &= mask
                    } else {
                        x = (x << (8 - shift)) & (mask as u16);
                        self.push(&[x as u8], bits);
                    }
                }
            } else {
                self.truncate(dst_len / 8);
                let dst = self.push(slice, bits);
                bits -= full_bytes * 8;
                if bits > 0 {
                    let mask = 0xFFu8 << (8 - bits);
                    dst[full_bytes] &= mask;
                }
            }
        }
        Ok(())
    }

    fn push(&mut self, slice: &[u8], bits: usize) -> &mut [u8] {
        let offset = self.length_in_bits() / 8;
        let length = slice.len().min(bits_to_bytes(bits));
        self.update_length_in_bits(bits);
        &mut self.extend(&slice[..length])[offset..]
    }
}

#[derive(Debug, Default, PartialEq, Clone, Eq)]
pub struct BuilderData {
    data: SmallData,
    length_in_bits: usize,
    pub(super) references: smallvec::SmallVec<[Cell; 4]>,
    pub(super) cell_type: CellType,
}

impl BuilderData {
    pub const fn default() -> Self {
        Self::new()
    }
    pub const fn new() -> Self {
        BuilderData {
            data: smallvec::SmallVec::new_const(),
            length_in_bits: 0,
            references: smallvec::SmallVec::new_const(),
            cell_type: CellType::Ordinary,
        }
    }

    pub fn with_raw(data: impl Into<SmallData>, length_in_bits: usize) -> Result<BuilderData> {
        let mut data = data.into();
        if length_in_bits > data.len() * 8 {
            fail!(ExceptionCode::FatalError)
        } else if length_in_bits > BuilderData::bits_capacity() {
            fail!(ExceptionCode::CellOverflow)
        }
        let data_shift = length_in_bits % 8;
        if data_shift == 0 {
            data.truncate(length_in_bits / 8);
        } else {
            data.truncate(1 + length_in_bits / 8);
            if let Some(last_byte) = data.last_mut() {
                *last_byte = (*last_byte >> (8 - data_shift)) << (8 - data_shift);
            }
        }
        data.reserve_exact(EXACT_CAPACITY - data.len());
        Ok(BuilderData {
            data,
            length_in_bits,
            references: smallvec::SmallVec::new(),
            cell_type: CellType::Ordinary,
        })
    }

    pub fn with_raw_and_refs(
        data: impl Into<SmallData>,
        length_in_bits: usize,
        refs: impl IntoIterator<Item = Cell>,
    ) -> Result<BuilderData> {
        let mut builder = BuilderData::with_raw(data, length_in_bits)?;
        builder.references = refs.into_iter().collect();
        Ok(builder)
    }

    pub fn with_bytes(data: impl AsRef<[u8]>) -> Result<BuilderData> {
        let data = data.as_ref();
        if data.is_empty() {
            return Ok(BuilderData::new());
        }
        let length_in_bits = data.len() * 8;
        if length_in_bits > BuilderData::bits_capacity() {
            fail!(ExceptionCode::CellOverflow)
        }
        BuilderData::with_raw(data, length_in_bits)
    }

    pub fn with_bitstring(data: impl Into<SmallData>) -> Result<BuilderData> {
        let data = data.into();
        let length_in_bits = find_tag(data.as_slice());
        if length_in_bits == 0 {
            Ok(BuilderData::new())
        } else if length_in_bits > data.len() * 8 {
            fail!(ExceptionCode::FatalError)
        } else if length_in_bits > BuilderData::bits_capacity() {
            fail!(ExceptionCode::CellOverflow)
        } else {
            BuilderData::with_raw(data, length_in_bits)
        }
    }

    pub fn with_ref(cell: Cell) -> BuilderData {
        let mut builder = BuilderData {
            data: smallvec::SmallVec::new_const(),
            length_in_bits: 0,
            references: smallvec::SmallVec::with_capacity(1),
            cell_type: CellType::Ordinary,
        };
        builder.references.push(cell);
        builder
    }

    /// finalize cell with default max depth
    pub fn into_cell(self) -> Result<Cell> {
        self.finalize(MAX_SAFE_DEPTH)
    }

    /// loads builder as bitstring to slice
    /// maximum length 1023 bits, type must be Ordinary, no references
    pub(super) fn into_bitstring(self) -> SliceData {
        debug_assert!(self.references.is_empty(), "builder should not have any references");
        SliceData::with_bitstring(self.data, self.length_in_bits)
    }

    /// use max_depth to limit depth
    pub fn finalize(mut self, max_depth: u16) -> Result<Cell> {
        let mut children_level_mask = LevelMask::with_level(0);
        for r in self.references.iter() {
            children_level_mask |= r.level_mask();
        }
        let level_mask = match self.cell_type {
            CellType::Unknown => fail!("failed to finalize a cell of unknown type"),
            CellType::Ordinary => children_level_mask,
            CellType::PrunedBranch => {
                if self.bits_used() < 16 {
                    fail!("failed to get level mask for pruned branch cell");
                }
                // mask validity gets checked later
                LevelMask::with_mask(self.data[1])
            }
            CellType::LibraryReference => LevelMask::with_level(0),
            CellType::MerkleProof => {
                if self.references.len() != 1 {
                    fail!(
                        "Merkle proof cell must have exactly one reference, but got {}",
                        self.references.len()
                    );
                }
                let hash = self.references()[0].hash(0);
                if hash.as_slice() != &self.data[1..33] {
                    fail!("Merkle proof cell reference hash does not match the hash in the data {:x} != {}",
                        hash, hex::encode(&self.data[1..33]));
                }
                children_level_mask.virtualize(1)
            }
            CellType::MerkleUpdate => {
                if self.references.len() != 2 {
                    fail!(
                        "Merkle update cell must have exactly two references, but got {}",
                        self.references.len()
                    );
                }
                let hash = self.references()[0].hash(0);
                if hash.as_slice() != &self.data[1..33] {
                    fail!(
                        "Merkle update cell first reference hash does not match \
                        the first hash in the data {:x} != {}",
                        hash,
                        hex::encode(&self.data[1..33])
                    );
                }
                let hash = self.references()[1].hash(0);
                if hash.as_slice() != &self.data[33..65] {
                    fail!(
                        "Merkle update cell second reference hash does not match \
                        the second hash in the data {:x} != {}",
                        hash,
                        hex::encode(&self.data[33..65])
                    );
                }
                children_level_mask.virtualize(1)
            }
        };
        append_tag(&mut self.data, self.length_in_bits);

        Ok(Cell::with_cell_impl(DataCell::with_params(
            self.references.to_vec(),
            &self.data,
            self.cell_type,
            level_mask.mask(),
            Some(max_depth),
        )?))
    }

    pub fn references(&self) -> &[Cell] {
        self.references.as_slice()
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn cell_type(&self) -> CellType {
        self.cell_type
    }

    pub fn compare_data(&self, other: &Self) -> Result<(Option<usize>, Option<usize>)> {
        if self == other {
            return Ok((None, None));
        }
        let label1 = SliceData::load_bitstring(self.clone())?;
        let label2 = SliceData::load_bitstring(other.clone())?;
        let (_prefix, rem1, rem2) = SliceData::common_prefix(&label1, &label2);
        // unwraps are safe because common_prefix returns None if slice is empty
        Ok((
            rem1.map(|rem| rem.get_bit(0).expect("check common_prefix function") as usize),
            rem2.map(|rem| rem.get_bit(0).expect("check common_prefix function") as usize),
        ))
    }

    pub fn from_cell(cell: &Cell) -> Result<BuilderData> {
        let data = smallvec::SmallVec::from_slice(cell.data());
        let mut builder = BuilderData::with_raw(data, cell.bit_length())?;
        builder.references = cell.clone_references();
        builder.cell_type = cell.cell_type();
        Ok(builder)
    }

    pub const fn length_in_bits(&self) -> usize {
        self.length_in_bits
    }

    pub fn can_append(&self, x: &BuilderData) -> bool {
        self.bits_free() >= x.bits_used() && self.references_free() >= x.references_used()
    }

    pub fn prepend_raw(&mut self, slice: &[u8], bits: usize) -> Result<&mut Self> {
        if bits != 0 {
            let mut buffer = BuilderData::with_raw(smallvec::SmallVec::from_slice(slice), bits)?;
            buffer.append_raw(self.data(), self.length_in_bits())?;
            self.length_in_bits = buffer.length_in_bits;
            self.data = buffer.data;
        }
        Ok(self)
    }

    pub fn append_raw(&mut self, slice: &[u8], bits: usize) -> Result<&mut Self> {
        let capacity = BuilderData::bits_capacity();
        self.append_raw_data(slice, bits, Some(capacity))?;
        debug_assert!(self.length_in_bits <= capacity);
        debug_assert!(self.data.len() <= bits_to_bytes(capacity));
        Ok(self)
    }

    pub fn checked_append_reference(&mut self, cell: Cell) -> Result<&mut Self> {
        if self.references_free() == 0 {
            fail!(ExceptionCode::CellOverflow)
        } else {
            self.references.push(cell);
            Ok(self)
        }
    }

    pub fn checked_prepend_reference(&mut self, cell: Cell) -> Result<&mut Self> {
        if self.references_free() == 0 {
            fail!(ExceptionCode::CellOverflow)
        } else {
            self.references.insert(0, cell);
            Ok(self)
        }
    }

    pub fn replace_data(&mut self, data: impl Into<SmallData>, length_in_bits: usize) {
        let data = data.into();
        self.length_in_bits = length_in_bits.min(MAX_DATA_BITS).min(data.len() * 8);
        self.data = data;
    }

    pub fn replace_reference_cell(&mut self, index: usize, child: Cell) {
        match self.references.get_mut(index) {
            None => {
                log::error!(
                    "replacing not existed cell by index {} with cell hash {:x}",
                    index,
                    child.repr_hash()
                );
            }
            Some(old) => *old = child,
        }
    }

    pub fn set_type(&mut self, cell_type: CellType) {
        self.cell_type = cell_type;
    }

    pub fn is_empty(&self) -> bool {
        self.length_in_bits() == 0 && self.references().is_empty()
    }

    pub fn trunc(&mut self, length_in_bits: usize) -> Result<()> {
        if self.length_in_bits < length_in_bits {
            fail!(ExceptionCode::FatalError)
        } else {
            self.length_in_bits = length_in_bits;
            self.data.truncate(1 + length_in_bits / 8);
            Ok(())
        }
    }
}

// use only for test purposes
#[cfg(test)]
impl BuilderData {
    pub(crate) fn append_reference(&mut self, child: BuilderData) {
        self.references.push(child.into_cell().unwrap());
    }
}

impl Display for BuilderData {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(
            f,
            "data: {} len: {} reference count: {}",
            hex::encode(&self.data),
            self.length_in_bits,
            self.references.len()
        )
    }
}

impl UpperHex for BuilderData {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(f, "{}", hex::encode_upper(&self.data))
    }
}

impl Binary for BuilderData {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        self.data.iter().try_for_each(|x| write!(f, "{:08b}", x))
    }
}

impl AppendBits for BuilderData {
    fn extend(&mut self, slice: &[u8]) -> &mut [u8] {
        self.data.extend_from_slice(slice);
        self.data.as_mut_slice()
    }
    fn length_in_bits(&self) -> usize {
        self.length_in_bits
    }
    fn truncate(&mut self, len: usize) {
        self.data.truncate(len);
    }
    fn update_length_in_bits(&mut self, bits: usize) {
        self.length_in_bits += bits;
    }
}

#[derive(Debug, Default, PartialEq, Clone, Eq)]
pub struct Bitstring {
    data: Vec<u8>,
    length_in_bits: usize,
}

impl Bitstring {
    pub fn data(&self) -> &[u8] {
        self.data.as_slice()
    }
    pub fn append_raw(&mut self, slice: &[u8], bits: usize) -> Result<()> {
        self.append_raw_data(slice, bits, None)
    }
}

impl AppendBits for Bitstring {
    fn extend(&mut self, slice: &[u8]) -> &mut [u8] {
        self.data.extend_from_slice(slice);
        self.data.as_mut_slice()
    }
    fn length_in_bits(&self) -> usize {
        self.length_in_bits
    }
    fn truncate(&mut self, len: usize) {
        self.data.truncate(len);
    }
    fn update_length_in_bits(&mut self, bits: usize) {
        self.length_in_bits += bits;
    }
}
