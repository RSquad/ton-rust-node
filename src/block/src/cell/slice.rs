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
    cell::{BuilderData, Cell, CellType, LevelMask, SmallData},
    error, fail, parse_slice_base,
    types::{bits_to_bytes, UInt256},
    ExceptionCode, Result,
};
use std::{
    cmp, fmt, hash, mem,
    ops::{Bound, Range, RangeBounds},
};

#[derive(Eq, PartialEq, Clone)]
enum InternalData {
    Cell(Cell),
    Data(SmallData, usize), // bitstring variant which optimizes storage of data without references
}

#[derive(Eq, Clone)]
pub struct SliceData {
    data: InternalData,
    data_window: Range<usize>,
    references_window: Range<usize>,
}

impl Default for SliceData {
    fn default() -> Self {
        Self::new_empty()
    }
}

impl PartialOrd for SliceData {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl hash::Hash for SliceData {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.get_bytestring(0).hash(state);
        for i in 0..self.remaining_references() {
            state.write(self.reference(i).unwrap().repr_hash().as_slice());
        }
    }
}

// compares only data, not references
impl Ord for SliceData {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.compare_bitstrings(other)
    }
}

impl PartialEq for SliceData {
    fn eq(&self, other: &SliceData) -> bool {
        let refs_count = self.remaining_references();
        if refs_count != other.remaining_references() {
            return false;
        }
        for i in 0..refs_count {
            let ref1 = self.reference(i).unwrap();
            let ref2 = other.reference(i).unwrap();
            if ref1 != ref2 {
                return false;
            }
        }
        self.cmp(other) == cmp::Ordering::Equal
    }
}

impl SliceData {
    pub const fn new_empty() -> SliceData {
        Self {
            data: InternalData::Data(SmallData::new_const(), 0),
            data_window: 0..0,
            references_window: 0..0,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self.data, InternalData::Data(_, 0))
    }

    pub const fn with_uint256(data: [u8; 128]) -> SliceData {
        SliceData {
            data: InternalData::Data(SmallData::from_const(data), 256),
            data_window: 0..256,
            references_window: 0..0,
        }
    }

    pub const ZERO_ID: SliceData = SliceData {
        data: InternalData::Data(SmallData::from_const([0; 128]), 256),
        data_window: 0..256,
        references_window: 0..0,
    };

    pub fn load_builder(builder: BuilderData) -> Result<SliceData> {
        if builder.cell_type() == CellType::PrunedBranch {
            fail!(ExceptionCode::PrunedCellAccess)
        }
        // if no references we can load bitstring
        SliceData::load_cell(builder.into_cell()?)
    }

    pub fn load_cell(cell: Cell) -> Result<SliceData> {
        if cell.is_pruned() {
            fail!(ExceptionCode::PrunedCellAccess)
        } else {
            Ok(SliceData {
                references_window: 0..cell.references_count(),
                data_window: 0..cell.bit_length(),
                data: InternalData::Cell(cell),
            })
        }
    }

    pub fn load_cell_with_window(
        cell: Cell,
        data_window: Range<usize>,
        references_window: Range<usize>,
    ) -> Result<SliceData> {
        if cell.is_pruned() {
            fail!(ExceptionCode::PrunedCellAccess)
        } else {
            let bits = cell.bit_length();
            let refs = cell.references_count();
            if data_window.end > bits {
                fail!(
                    ExceptionCode::CellUnderflow,
                    "data window end {} exceeds cell bits {bits}",
                    data_window.end
                )
            }
            if references_window.end > refs {
                fail!(
                    ExceptionCode::CellUnderflow,
                    "references window end {} exceeds cell references {refs}",
                    references_window.end
                )
            }
            Ok(SliceData { references_window, data_window, data: InternalData::Cell(cell) })
        }
    }

    pub fn with_bitstring(data: impl Into<SmallData>, length_in_bits: usize) -> Self {
        Self {
            data: InternalData::Data(data.into(), length_in_bits.min(super::MAX_DATA_BITS)),
            references_window: 0..0,
            data_window: 0..length_in_bits,
        }
    }

    pub fn with_reference(cell: Cell) -> Result<Self> {
        let mut builder = BuilderData::new();
        builder.checked_append_reference(cell)?;
        SliceData::load_builder(builder)
    }

    pub fn load_bitstring(builder: BuilderData) -> Result<SliceData> {
        if builder.cell_type != CellType::Ordinary {
            fail!("cell type should be ordinary but it is {}", builder.cell_type)
        }
        if builder.length_in_bits() > super::MAX_DATA_BITS {
            fail!(
                "length should be less or equal to {} but it is {}",
                super::MAX_DATA_BITS,
                builder.length_in_bits()
            )
        }
        if builder.references_used() != 0 {
            fail!("should not have any references but it has {}", builder.references_used())
        }
        Ok(builder.into_bitstring())
    }

    pub fn load_cell_ref(cell: &Cell) -> Result<SliceData> {
        SliceData::load_cell(cell.clone())
    }

    pub fn from_string(value: &str) -> Result<SliceData> {
        let vec =
            parse_slice_base(value, 0, 16).ok_or_else(|| error!(ExceptionCode::FatalError))?;
        SliceData::load_bitstring(BuilderData::with_bitstring(vec)?)
    }

    pub fn remaining_references(&self) -> usize {
        if self.references_window.start >= self.references_window.end {
            return 0;
        }
        self.references_window.end - self.references_window.start
    }

    pub const fn remaining_bits(&self) -> usize {
        if self.data_window.start > self.data_window.end {
            return 0;
        }
        self.data_window.end - self.data_window.start
    }

    pub fn remainig(&self) -> (usize, usize) {
        (self.remaining_bits(), self.remaining_references())
    }

    pub fn compare_bitstrings(&self, other: &SliceData) -> cmp::Ordering {
        let bits = self.remaining_bits();
        let ordering = bits.cmp(&other.remaining_bits());
        if ordering != cmp::Ordering::Equal {
            return ordering;
        }
        // optimization for byte aligned data
        if bits.is_multiple_of(8) {
            let bytes = bits_to_bytes(bits);
            if self.data_window.start.is_multiple_of(8) {
                let q1 = self.data_window.start / 8;
                if other.data_window.start.is_multiple_of(8) {
                    let q2 = other.data_window.start / 8;
                    // both start from byte boundary
                    return self.storage()[q1..q1 + bytes].cmp(&other.storage()[q2..q2 + bytes]);
                } else {
                    let vec = other.get_bytestring(0);
                    // only self starts from byte boundary
                    return self.storage()[q1..q1 + bytes].cmp(&vec);
                }
            } else if other.data_window.start.is_multiple_of(8) {
                let vec = self.get_bytestring(0);
                let q2 = other.data_window.start / 8;
                // only other starts from byte boundary
                return vec.as_slice().cmp(&other.storage()[q2..q2 + bytes]);
            }
        }
        let vec1 = self.get_bytestring(0);
        let vec2 = other.get_bytestring(0);
        vec1.cmp(&vec2)
    }

    /// shrinks data and referenveces: ranges - subranges of current windows
    /// Returns remaining data and references as a slice
    pub fn shrink(
        &mut self,
        range_data: impl RangeBounds<usize>,
        range_refs: impl RangeBounds<usize>,
    ) -> SliceData {
        let mut data_window = 0..0;
        let mut references_window = 0..0;
        let data_len = self.remaining_bits();
        let start = match range_data.start_bound() {
            Bound::Included(start) => *start,
            Bound::Excluded(start) => start + 1,
            Bound::Unbounded => 0,
        };
        let end = match range_data.end_bound() {
            Bound::Included(end) => end + 1,
            Bound::Excluded(end) => *end,
            Bound::Unbounded => data_len,
        };
        if (start <= end) && (end <= data_len) {
            let start = self.data_window.start + start;
            let end = self.data_window.start + end;
            if start != self.data_window.start {
                // return prefix
                data_window.start = self.data_window.start;
                data_window.end = start;
            } else {
                // return suffix
                data_window.start = end;
                data_window.end = self.data_window.end;
            }
            self.data_window.start = start;
            self.data_window.end = end;
        }
        let refs_count = self.remaining_references();
        let start = match range_refs.start_bound() {
            Bound::Included(start) => *start,
            Bound::Excluded(start) => start + 1,
            Bound::Unbounded => 0,
        };
        let end = match range_refs.end_bound() {
            Bound::Included(end) => end + 1,
            Bound::Excluded(end) => *end,
            Bound::Unbounded => refs_count,
        };
        if (start <= end) && (end <= refs_count) {
            let start = self.references_window.start + start;
            let end = self.references_window.start + end;
            if start != self.references_window.start {
                // return prefix
                references_window.start = self.references_window.start;
                references_window.end = start;
            } else {
                // return suffix
                references_window.start = end;
                references_window.end = self.references_window.end;
            }
            self.references_window.start = start;
            self.references_window.end = end;
        }
        if data_window.end != self.data_window.start {
            assert_eq!(data_window.start, self.data_window.end);
        }
        if references_window.end != self.references_window.start {
            assert_eq!(references_window.start, self.references_window.end);
        }
        SliceData { data: self.data.clone(), data_window, references_window }
    }

    /// shrinks data_window: range - subrange of current window, returns prefix of suffix
    pub fn shrink_data<T: RangeBounds<usize>>(&mut self, range: T) -> SliceData {
        let mut data_window = 0..0;
        let data_len = self.remaining_bits();
        let start = match range.start_bound() {
            Bound::Included(start) => *start,
            Bound::Excluded(start) => start + 1,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Included(end) => end + 1,
            Bound::Excluded(end) => *end,
            Bound::Unbounded => data_len,
        };
        if (start <= end) && (end <= data_len) {
            let start = self.data_window.start + start;
            let end = self.data_window.start + end;
            if start != 0 {
                // return prefix
                data_window.start = self.data_window.start;
                data_window.end = start;
            } else {
                // return suffix
                data_window.start = end;
                data_window.end = self.data_window.end;
            }
            self.data_window.start = start;
            self.data_window.end = end;
        }
        SliceData { data: self.data.clone(), data_window, references_window: 0..0 }
    }

    /// shrinks references_window: range - subrange of current window, returns shrinked references
    pub fn shrink_references<T: RangeBounds<usize>>(&mut self, range: T) -> Vec<Cell> {
        let mut vec = vec![];
        if let InternalData::Cell(cell) = &self.data {
            let refs_count = self.remaining_references();
            let start = match range.start_bound() {
                Bound::Included(start) => *start,
                Bound::Excluded(start) => start + 1,
                Bound::Unbounded => 0,
            };
            let end = match range.end_bound() {
                Bound::Included(end) => end + 1,
                Bound::Excluded(end) => *end,
                Bound::Unbounded => refs_count,
            };

            if (start <= end) && (end <= refs_count) {
                (0..start).for_each(|i| vec.push(cell.reference(i).unwrap()));
                (end..refs_count).for_each(|i| vec.push(cell.reference(i).unwrap()));
                self.references_window.end = self.references_window.start + end;
                self.references_window.start += start;
            }
        }
        vec
    }

    pub fn clear_all_bits(&mut self) {
        self.data_window.start = self.data_window.end
    }

    pub fn clear_all_references(&mut self) {
        self.references_window.end = self.references_window.start
    }

    fn remaining_data(self) -> Result<BuilderData> {
        if self.data_window.start >= self.data_window.end {
            return Ok(BuilderData::new());
        }
        let length_in_bits = self.data_window.end - self.data_window.start;
        let (start, trailing) = (self.data_window.start / 8, self.data_window.start % 8);
        if trailing == 0 {
            match self.data {
                InternalData::Data(data, _) => {
                    if self.data_window.start == 0 {
                        BuilderData::with_raw(data, length_in_bits)
                    } else {
                        BuilderData::with_raw(
                            smallvec::SmallVec::from_slice(&data[start..]),
                            length_in_bits,
                        )
                    }
                }
                InternalData::Cell(cell) => BuilderData::with_raw(
                    smallvec::SmallVec::from_slice(&cell.data()[start..]),
                    length_in_bits,
                ),
            }
        } else if let Some(bits) = (length_in_bits + trailing).checked_sub(8) {
            match self.data {
                InternalData::Data(mut data, _length_in_bits) => {
                    data[0] = data[start] << trailing;
                    let mut builder = BuilderData::with_raw(data.clone(), 8 - trailing)?;
                    builder.append_raw(&data[start + 1..], bits)?;
                    Ok(builder)
                }
                InternalData::Cell(cell) => {
                    let mut data = smallvec::SmallVec::from_slice(&cell.data()[start..]);
                    data[0] <<= trailing;
                    let mut builder = BuilderData::with_raw(data, 8 - trailing)?;
                    builder.append_raw(&cell.data()[start + 1..], bits)?;
                    Ok(builder)
                }
            }
        } else {
            // less than one byte
            let mut data = match self.data {
                InternalData::Data(data, _length_in_bits) => data,
                InternalData::Cell(cell) => smallvec::SmallVec::from_slice(cell.data()),
            };
            data[0] = data[start] << trailing;
            BuilderData::with_raw(data, length_in_bits)
        }
    }

    pub fn shrink_by_remainder(&mut self, other: &SliceData) {
        if self.data_window.start <= other.data_window.start {
            self.data_window.end = other.data_window.start
        }
        if self.references_window.start <= other.references_window.start {
            self.references_window.end = other.references_window.start
        }
    }

    /// trim zeros from right to first one
    pub fn trim_right(&mut self) {
        for offset in (0..self.remaining_bits()).rev() {
            if self.get_bit_opt(offset) == Some(true) {
                self.data_window.end = self.data_window.start + offset;
                break;
            }
        }
    }

    fn check_remaining_bits(&self, bits: usize) -> Result<()> {
        if bits > self.remaining_bits() {
            fail!(
                ExceptionCode::CellUnderflow,
                "not enough bits to read {bits} > {}",
                self.remaining_bits()
            )
        }
        Ok(())
    }

    pub fn reference(&self, i: usize) -> Result<Cell> {
        if self.references_window.start + i < self.references_window.end {
            if let InternalData::Cell(cell) = &self.data {
                return cell.reference(self.references_window.start + i);
            }
        }
        fail!(
            ExceptionCode::CellUnderflow,
            "not enough references to read {} max: {}",
            i + 1,
            self.remaining_references()
        )
    }

    pub fn reference_opt(&self, i: usize) -> Option<Cell> {
        if self.references_window.start + i < self.references_window.end {
            if let InternalData::Cell(cell) = &self.data {
                return cell.reference(self.references_window.start + i).ok();
            }
        }
        None
    }

    pub fn storage(&self) -> &[u8] {
        match &self.data {
            InternalData::Data(data, _length_in_bits) => data,
            InternalData::Cell(cell) => cell.data(),
        }
    }

    /// returns internal cell regardless window settings
    /// use this function carefully
    /// it may create new real cell if SliceData was a bitstring
    pub fn cell(&self) -> Result<Cell> {
        match &self.data {
            InternalData::Cell(cell) => Ok(cell.clone()),
            _ => self.as_builder()?.into_cell(),
        }
    }

    /// returns internal cell regardless window settings
    /// don't use this function
    pub fn cell_opt(&self) -> Option<&Cell> {
        match &self.data {
            InternalData::Cell(cell) => Some(cell),
            _ => None,
        }
    }

    /// constructs new cell trunking original regarding window settings
    pub fn into_cell(self) -> Result<Cell> {
        if let InternalData::Cell(cell) = &self.data {
            if self.data_window.start == 0
                && self.data_window.end == cell.bit_length()
                && self.references_window.start == 0
                && self.references_window.end == cell.references_count()
            {
                return Ok(cell.clone());
            }
        }
        self.into_builder()?.into_cell()
    }

    /// constructs builder trunking original cell regarding window settings
    pub fn into_builder(self) -> Result<BuilderData> {
        let cell_type = self.cell_type();
        let slice = &self;
        let refs =
            (0..self.remaining_references()).map(|index| slice.reference(index).unwrap()).collect();
        let mut builder = self.remaining_data()?;
        builder.cell_type = cell_type;
        builder.references = refs;
        Ok(builder)
    }

    pub fn as_builder(&self) -> Result<BuilderData> {
        self.clone().into_builder()
    }

    pub fn checked_drain_reference(&mut self) -> Result<Cell> {
        if let InternalData::Cell(cell) = &self.data {
            if self.references_window.start < self.references_window.end {
                self.references_window.start += 1;
                return cell.reference(self.references_window.start - 1);
            }
        }
        fail!(ExceptionCode::CellUnderflow)
    }

    pub fn get_references(&self) -> Range<usize> {
        self.references_window.clone()
    }

    /// Returns subslice of current slice
    pub fn get_slice(&self, bits: usize) -> Result<SliceData> {
        self.check_remaining_bits(bits)?;
        let start = self.data_window.start;
        Ok(SliceData {
            data: self.data.clone(),
            data_window: start..start + bits,
            references_window: 0..0,
        })
    }

    pub fn get_bit_opt(&self, offset: usize) -> Option<bool> {
        if offset >= self.remaining_bits() {
            None
        } else {
            let index = self.data_window.start + offset;
            let q = index / 8;
            let r = index % 8;
            Some(((self.storage()[q] >> (7 - r)) & 1) != 0)
        }
    }

    pub fn get_bit(&self, offset: usize) -> Result<bool> {
        self.get_bit_opt(offset).ok_or_else(|| error!(ExceptionCode::CellUnderflow))
    }

    pub fn get_bits(&self, offset: usize, bits: usize) -> Result<u8> {
        self.check_remaining_bits(bits)?;
        if bits == 0 || bits > 8 {
            fail!(ExceptionCode::RangeCheckError, "bits should be in range 1..=8 but it is {bits}")
        }
        let data = self.storage();
        let index = self.data_window.start + offset;
        let q = index / 8;
        let r = index % 8;
        if r == 0 {
            Ok(data[q] >> (8 - bits))
        } else if bits <= (8 - r) {
            Ok((data[q] >> (8 - r - bits)) & ((1 << bits) - 1))
        } else {
            // We shall have here at least two bytes to read
            let ret = u16::from_be_bytes([data[q], data[q + 1]]);
            Ok((ret >> (8 - r)) as u8 >> (8 - bits))
        }
    }

    pub fn get_byte(&self, offset: usize) -> Result<u8> {
        self.get_bits(offset, 8)
    }

    pub fn get_next_bits(&mut self, bits: usize) -> Result<Vec<u8>> {
        let mut buffer;
        if bits <= 8 {
            let byte = self.get_bits(0, bits)?;
            buffer = vec![byte << (8 - bits)];
        } else {
            self.check_remaining_bits(bits)?;
            let bytes = bits_to_bytes(bits);
            buffer = vec![0u8; bytes];
            self.get_bits_to_slice(&mut buffer, Some((0, bits)));
        }
        self.data_window.start += bits;
        Ok(buffer)
    }

    pub fn get_next_bit(&mut self) -> Result<bool> {
        self.check_remaining_bits(1)?;
        let bit = self.get_bit(0)?;
        self.data_window.start += 1;
        Ok(bit)
    }

    pub fn get_next_bit_int(&mut self) -> Result<usize> {
        Ok(self.get_next_bit_opt().ok_or(ExceptionCode::CellUnderflow)?)
    }

    pub fn get_next_bit_opt(&mut self) -> Option<usize> {
        self.check_remaining_bits(1).ok()?;
        let bit = self.get_bit_opt(0)?;
        self.data_window.start += 1;
        Some(bit as usize)
    }

    pub fn get_next_byte(&mut self) -> Result<u8> {
        let value = self.get_byte(0)?;
        self.data_window.start += 8;
        Ok(value)
    }

    pub fn get_next_int(&mut self, bits: usize) -> Result<u64> {
        let value = self.get_int(bits)?;
        self.data_window.start += bits;
        Ok(value)
    }

    /// Reads integer from the specified number of bits without moving pointer
    /// Bits must be in range 0..=64
    /// If bits == 0 then returns 0
    /// If bits > 64 then returns error
    /// If not enough bits to read then returns error
    /// Interpretation is big-endian
    pub fn get_int(&self, bits: usize) -> Result<u64> {
        self.check_remaining_bits(bits)?;
        if bits == 0 {
            return Ok(0);
        }
        if bits > 64 {
            fail!("too many bits {} > 64", bits)
        }
        let mut buffer = [0u8; 8];
        let bytes = bits_to_bytes(bits);
        self.get_bits_to_slice(&mut buffer[..bytes], Some((0, bits)));
        Ok(u64::from_be_bytes(buffer) >> (64 - bits))
    }

    /// base function to read bytes into slice without moving pointer
    /// it will not check remainig bits
    fn get_bits_to_slice(&self, buffer: &mut [u8], offset_bits: Option<(usize, usize)>) {
        let (offset, bits) = offset_bits.unwrap_or_default();
        let data = self.storage();
        let start = self.data_window.start + offset;
        let mut q = start / 8;
        let r = start % 8;
        if r == 0 {
            buffer.copy_from_slice(&data[q..q + buffer.len()]);
        } else {
            for b in buffer.iter_mut() {
                let mut ret = (data[q] as u16) << 8;
                if let Some(b) = data.get(q + 1) {
                    ret |= *b as u16;
                }
                *b = (ret >> (8 - r)) as u8;
                q += 1;
            }
        }
        let r = bits % 8;
        if r != 0 {
            if let Some(last) = buffer.last_mut() {
                *last &= 0xFF << (8 - r);
            }
        }
    }

    fn get_const_bytes<const LEN: usize>(&mut self) -> Result<[u8; LEN]> {
        self.check_remaining_bits(LEN * 8)?;
        let mut result = [0u8; LEN];
        self.get_bits_to_slice(&mut result, None);
        Ok(result)
    }

    fn get_next_const_bytes<const LEN: usize>(&mut self) -> Result<[u8; LEN]> {
        let result = self.get_const_bytes::<LEN>()?;
        self.data_window.start += LEN * 8;
        Ok(result)
    }

    pub fn get_next_size(&mut self, max_value: usize) -> Result<u64> {
        if max_value == 0 {
            return Ok(0);
        }
        let bits = 16 - (max_value as u16).leading_zeros() as usize;
        self.get_next_int(bits)
    }

    pub fn get_next_u16(&mut self) -> Result<u16> {
        self.get_next_const_bytes::<{ mem::size_of::<u16>() }>().map(u16::from_be_bytes)
    }

    pub fn get_next_i16(&mut self) -> Result<i16> {
        self.get_next_const_bytes::<{ mem::size_of::<i16>() }>().map(i16::from_be_bytes)
    }

    pub fn get_next_u32(&mut self) -> Result<u32> {
        self.get_next_const_bytes::<{ mem::size_of::<u32>() }>().map(u32::from_be_bytes)
    }

    pub fn get_next_i32(&mut self) -> Result<i32> {
        self.get_next_const_bytes::<{ mem::size_of::<i32>() }>().map(i32::from_be_bytes)
    }

    pub fn get_next_i64(&mut self) -> Result<i64> {
        self.get_next_const_bytes::<{ mem::size_of::<i64>() }>().map(i64::from_be_bytes)
    }

    pub fn get_next_u64(&mut self) -> Result<u64> {
        self.get_next_const_bytes::<{ mem::size_of::<u64>() }>().map(u64::from_be_bytes)
    }

    pub fn get_next_u128(&mut self) -> Result<u128> {
        self.get_next_const_bytes::<{ mem::size_of::<u128>() }>().map(u128::from_be_bytes)
    }

    pub fn get_next_u256(&mut self) -> Result<[u8; 32]> {
        self.get_next_const_bytes::<32>()
    }

    pub fn get_next_hash(&mut self) -> Result<UInt256> {
        self.get_next_u256().map(UInt256::from)
    }

    pub fn get_next_bytes(&mut self, bytes: usize) -> Result<Vec<u8>> {
        let bits = bytes * 8;
        self.check_remaining_bits(bits)?;
        let mut buffer = vec![0; bytes];
        self.get_bits_to_slice(&mut buffer, None);
        self.data_window.start += bits;
        Ok(buffer)
    }

    pub fn get_bytes_to_slice(&self, buffer: &mut [u8]) -> Result<usize> {
        let bytes = buffer.len();
        let bits = bytes * 8;
        self.check_remaining_bits(bits)?;
        self.get_bits_to_slice(buffer, None);
        Ok(bits)
    }

    pub fn get_next_bytes_to_slice(&mut self, buffer: &mut [u8]) -> Result<()> {
        let bits = self.get_bytes_to_slice(buffer)?;
        self.data_window.start += bits;
        Ok(())
    }

    pub fn get_bytestring(&self, offset: usize) -> Vec<u8> {
        if self.remaining_bits() <= offset {
            return vec![];
        }
        let bits = self.remaining_bits() - offset;
        let bytes = bits_to_bytes(bits);
        let mut buffer = vec![0; bytes];
        self.get_bits_to_slice(&mut buffer, Some((offset, bits)));
        buffer
    }

    /// Returns Cell from references if present and next bit in slice is one
    pub fn get_next_maybe_reference(&mut self) -> Result<Option<Cell>> {
        if self.get_next_bit()? {
            let cell = self.checked_drain_reference()?;
            Ok(Some(cell))
        } else {
            Ok(None)
        }
    }

    /// Returns Cell from references if present and next bit in slice is one
    pub fn get_next_dictionary(&mut self) -> Result<Option<Cell>> {
        self.get_next_maybe_reference()
    }

    /// Returns subslice of current slice and moves pointer
    pub fn get_next_slice(&mut self, bits: usize) -> Result<SliceData> {
        self.check_remaining_bits(bits)?;
        let data_window = self.data_window.start..self.data_window.start + bits;
        self.data_window.start += bits;
        Ok(SliceData { data: self.data.clone(), data_window, references_window: 0..0 })
    }

    /// Returns true if no more bits
    pub fn is_empty_bitstring(&self) -> bool {
        self.data_window.start >= self.data_window.end
    }

    /// Returns true if no more bits and refs
    pub fn is_empty_cell(&self) -> bool {
        self.references_window.start >= self.references_window.end
            && self.data_window.start >= self.data_window.end
    }

    pub fn move_by(&mut self, offset: usize) -> Result<()> {
        self.check_remaining_bits(offset)?;
        self.data_window.start += offset;
        Ok(())
    }

    pub fn pos(&self) -> usize {
        self.data_window.start
    }

    /// returns false if prefix is not fully in self
    pub fn erase_prefix(&mut self, prefix: &SliceData) -> bool {
        if self.is_empty_bitstring() || (self.remaining_bits() < prefix.remaining_bits()) {
            false
        } else if prefix.is_empty_bitstring() {
            true
        } else if *self == *prefix {
            self.clear_all_bits();
            true
        } else {
            match SliceData::common_prefix(self, prefix) {
                (_, _, Some(_)) => false, // prefix should be fully in self
                (_, Some(remainder), _) => {
                    *self = remainder;
                    true
                }
                (_, None, _) => {
                    log::warn!(target: "tvm", "unreachable in erase_prefix {} {}", self, prefix);
                    self.clear_all_bits();
                    true
                }
            }
        }
    }

    pub fn common_prefix(
        a: &SliceData,
        b: &SliceData,
    ) -> (Option<SliceData>, Option<SliceData>, Option<SliceData>) {
        let mut offset = 0;
        let max_possible_prefix_length_in_bits = a.remaining_bits().min(b.remaining_bits());
        while (offset + 8) <= max_possible_prefix_length_in_bits {
            if a.get_byte(offset).unwrap() != b.get_byte(offset).unwrap() {
                break;
            }
            offset += 8;
        }
        let (prefix, common_length) = if offset < max_possible_prefix_length_in_bits {
            let last_bits_len = (max_possible_prefix_length_in_bits - offset).min(8);
            let a_bits = a.get_bits(offset, last_bits_len).unwrap();
            let b_bits = b.get_bits(offset, last_bits_len).unwrap();
            let diff = a_bits ^ b_bits;
            let diff = (diff.leading_zeros() as usize - (8 - last_bits_len)).min(last_bits_len);

            (a, offset + diff)
        } else if a.remaining_bits() < b.remaining_bits() {
            (a, max_possible_prefix_length_in_bits)
        } else {
            (b, max_possible_prefix_length_in_bits)
        };
        if common_length == 0 {
            (
                None,
                (a.remaining_bits() != 0).then(|| a.clone()),
                (b.remaining_bits() != 0).then(|| b.clone()),
            )
        } else {
            let rem_a = (a.remaining_bits() != common_length).then(|| {
                let mut a = a.clone();
                a.data_window.start += common_length;
                a
            });
            let rem_b = (b.remaining_bits() != common_length).then(|| {
                let mut b = b.clone();
                b.data_window.start += common_length;
                b
            });
            let mut prefix = prefix.clone();
            prefix.references_window.end = prefix.references_window.start;
            prefix.data_window.end = prefix.data_window.start + common_length;
            (Some(prefix), rem_a, rem_b)
        }
    }

    pub fn overwrite_prefix(&mut self, prefix: &SliceData) -> Result<()> {
        if prefix.is_empty_bitstring() {
            Ok(())
        } else if let Some(length) = self.remaining_bits().checked_sub(prefix.remaining_bits()) {
            let mut builder = prefix.as_builder()?;
            let suffix = self.get_bytestring(builder.length_in_bits());
            builder.append_raw(&suffix, length)?;
            *self = SliceData::load_bitstring(builder)?;
            Ok(())
        } else {
            fail!("Prefix should not be longer than self")
        }
    }

    pub fn cell_type(&self) -> CellType {
        match &self.data {
            InternalData::Cell(cell) => cell.cell_type(),
            _ => Default::default(),
        }
    }
    pub fn level(&self) -> u8 {
        match &self.data {
            InternalData::Cell(cell) => cell.level(),
            _ => Default::default(),
        }
    }
    pub fn level_mask(&self) -> LevelMask {
        match &self.data {
            InternalData::Cell(cell) => cell.level_mask(),
            _ => Default::default(),
        }
    }

    /// Returns cell's higher hash for given index (last one - representation hash)
    pub fn hash(&self, index: usize) -> UInt256 {
        match &self.data {
            InternalData::Cell(cell) => Cell::hash(cell, index),
            _ => Default::default(),
        }
    }

    /// Returns cell's representation hash
    pub fn repr_hash(&self) -> UInt256 {
        match &self.data {
            InternalData::Cell(cell) => cell.repr_hash(),
            _ => Default::default(),
        }
    }

    /// Returns cell's depth for given index
    pub fn depth(&self, index: usize) -> u16 {
        match &self.data {
            InternalData::Cell(cell) => cell.depth(index),
            _ => Default::default(),
        }
    }

    /// Returns cell's hashes (representation and highers)
    pub fn hashes(&self) -> Vec<UInt256> {
        match &self.data {
            InternalData::Cell(cell) => cell.hashes(),
            _ => Default::default(),
        }
    }

    /// Returns cell's depth (for current state and each level)
    pub fn depths(&self) -> Vec<u16> {
        match &self.data {
            InternalData::Cell(cell) => cell.depths(),
            _ => Default::default(),
        }
    }

    pub fn virtualization(&self) -> u8 {
        match &self.data {
            InternalData::Cell(cell) => cell.virtualization(),
            _ => 0,
        }
    }

    pub fn as_hex_string(&self) -> String {
        let len = self.remaining_bits();
        let mut data = self.get_bytestring(0);
        if len.is_multiple_of(8) {
            data.push(0x80);
            super::to_hex_string(data, len, true)
        } else {
            let mut data: SmallData = data.into();
            super::append_tag(&mut data, len);
            super::to_hex_string(data, len, true)
        }
    }

    #[cfg(test)]
    fn is_full_cell_slice(&self) -> bool {
        match &self.data {
            InternalData::Cell(cell) => {
                self.data_window.start == 0
                    && self.data_window.end == cell.bit_length()
                    && self.references_window.start == 0
                    && self.references_window.end == cell.references_count()
            }
            InternalData::Data(_data, length_in_bits) => {
                self.data_window.start == 0
                    && self.data_window.end == *length_in_bits
                    && self.references_window.start == 0
                    && self.references_window.end == 0
            }
        }
    }

    /// Returns true if slice contains exactly given bytes
    pub fn contains_bytes(&self, bytes: impl AsRef<[u8]>) -> bool {
        let bytes = bytes.as_ref();
        let len = bytes.len();
        if self.remaining_bits() != len * 8 {
            return false;
        }
        if self.data_window.start.is_multiple_of(8) {
            let start = self.data_window.start / 8;
            return &self.storage()[start..start + len] == bytes;
        }
        let mut offset = 0;
        for b in bytes {
            if self.get_byte(offset).unwrap_or_default() != *b {
                return false;
            }
            offset += 8;
        }
        true
    }
}

/// subject to move to tests
/// it used from other repos
/// need task
impl SliceData {
    pub fn new(data: Vec<u8>) -> SliceData {
        match crate::find_tag(data.as_slice()) {
            0 => SliceData::default(),
            length_in_bits => SliceData::from_raw(data, length_in_bits),
        }
    }

    pub fn from_raw(data: impl Into<SmallData>, length_in_bits: usize) -> SliceData {
        SliceData::load_bitstring(BuilderData::with_raw(data, length_in_bits).unwrap()).unwrap()
    }

    pub fn append_reference(&mut self, other: SliceData) -> Result<&mut SliceData> {
        let mut builder = self.as_builder()?;
        builder.checked_append_reference(other.into_cell()?)?;
        *self = SliceData::load_builder(builder).expect("it should be used only in tests");
        Ok(self)
    }

    pub fn withdraw(&mut self) -> SliceData {
        std::mem::take(self)
    }
}

impl fmt::Debug for SliceData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self)
    }
}

impl fmt::Display for SliceData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "data: {}..{}, references: {}..{}, data slice: {}",
            self.data_window.start,
            self.data_window.end,
            self.references_window.start,
            self.references_window.end,
            hex::encode(self.get_bytestring(0)),
        )?;
        match &self.data {
            InternalData::Cell(cell) => writeln!(f, "cell hash: {:x}", cell.repr_hash()),
            InternalData::Data(data, length_in_bits) => {
                writeln!(f, "cell: {} - {length_in_bits}", hex::encode(data))
            }
        }
    }
}

impl fmt::LowerHex for SliceData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.as_hex_string())
    }
}

impl fmt::UpperHex for SliceData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let len = self.remaining_bits();
        let mut data: SmallData = self.get_bytestring(0).into();
        super::append_tag(&mut data, len);
        write!(f, "{}", super::to_hex_string(data.as_slice(), len, false))
    }
}

#[cfg(test)]
#[path = "tests/test_slice.rs"]
mod tests;
