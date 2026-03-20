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
use crate::{error, fail, ExceptionCode, Result, Sha256, UInt256};
#[cfg(feature = "cell_counter")]
use std::sync::atomic::{AtomicU64, Ordering};
use std::{
    any::Any,
    cmp::{max, min},
    collections::HashSet,
    convert::TryInto,
    fmt::{self, Display, Formatter},
    ops::{BitOr, BitOrAssign, Deref},
    sync::{Arc, LazyLock, Weak},
};

pub const SHA256_SIZE: usize = 32;
pub const DEPTH_SIZE: usize = 2;
pub const MAX_REFERENCES_COUNT: usize = 4;
pub const MAX_DATA_BITS: usize = 1023;
pub const MAX_DATA_BYTES: usize = 128; // including tag
pub const MAX_BIG_DATA_BYTES: usize = 0xff_ff_ff; // 1024 * 1024 * 16 - 1
pub const MAX_LEVEL: usize = 3;
pub const MAX_LEVEL_MASK: u8 = 7;
pub const MAX_DEPTH: u16 = u16::MAX - 1;

// recommended maximum depth, this value is safe for stack. Use custom stack size
// to use bigger depths (see `test_max_depth`).
pub const MAX_SAFE_DEPTH: u16 = 2048;

#[derive(
    Debug,
    Default,
    Eq,
    PartialEq,
    Clone,
    Copy,
    Hash,
    num_derive::FromPrimitive,
    num_derive::ToPrimitive,
)]
pub enum CellType {
    Unknown,
    #[default]
    Ordinary,
    PrunedBranch,
    LibraryReference,
    MerkleProof,
    MerkleUpdate,
}

#[derive(Debug, Default, Eq, PartialEq, Clone, Copy, Hash)]
pub struct LevelMask(u8);

impl LevelMask {
    pub fn with_level(level: u8) -> Self {
        LevelMask(match level {
            0 => 0,
            1 => 1,
            2 => 3,
            3 => 7,
            _ => {
                log::error!("{} {}", file!(), line!());
                0
            }
        })
    }

    pub fn is_valid(mask: u8) -> bool {
        mask <= 7
    }

    pub fn with_mask(mask: u8) -> Self {
        if Self::is_valid(mask) {
            LevelMask(mask)
        } else {
            log::error!("{} {}", file!(), line!());
            LevelMask(0)
        }
    }

    pub fn for_merkle_cell(children_mask: LevelMask) -> Self {
        LevelMask(children_mask.0 >> 1)
    }

    pub fn level(&self) -> u8 {
        if !Self::is_valid(self.0) {
            log::error!("{} {}", file!(), line!());
            255
        } else {
            // count of set bits (low three)
            (self.0 & 1) + ((self.0 >> 1) & 1) + ((self.0 >> 2) & 1)
        }
    }

    pub fn mask(&self) -> u8 {
        self.0
    }

    // if cell contains required hash() - it will be returned,
    // else = max avaliable, but less then index
    //
    // rows - cell mask
    //       0(0)  1(1)  2(3)  3(7)  columns - index(mask)
    // 000     0     0     0     0     cells - index(AND result)
    // 001     0     1(1)  1(1)  1(1)
    // 010     0     0(0)  1(2)  1(2)
    // 011     0     1(1)  2(3)  2(3)
    // 100     0     0(0)  0(0)  1(4)
    // 101     0     1(1)  0(0)  2(5)
    // 110     0     0(0)  1(2)  2(6)
    // 111     0     1(1)  2(3)  3(7)
    pub fn calc_hash_index(&self, mut index: usize) -> usize {
        index = min(index, 3);
        LevelMask::with_mask(self.0 & LevelMask::with_level(index as u8).0).level() as usize
    }

    pub fn calc_virtual_hash_index(&self, index: usize, virt_offset: u8) -> usize {
        LevelMask::with_mask(self.0 >> virt_offset).calc_hash_index(index)
    }

    pub fn virtualize(&self, virt_offset: u8) -> Self {
        LevelMask::with_mask(self.0 >> virt_offset)
    }

    pub fn is_significant_index(&self, index: usize) -> bool {
        index == 0 || self.0 & LevelMask::with_level(index as u8).0 != 0
    }
}

impl BitOr for LevelMask {
    type Output = Self;

    // rhs is the "right-hand side" of the expression `a | b`
    fn bitor(self, rhs: Self) -> Self {
        LevelMask::with_mask(self.0 | rhs.0)
    }
}

impl BitOrAssign for LevelMask {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl Display for LevelMask {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "{:03b}", self.0)
    }
}

impl TryFrom<u8> for CellType {
    type Error = crate::Error;
    fn try_from(num: u8) -> Result<CellType> {
        let typ = match num {
            1 => CellType::PrunedBranch,
            2 => CellType::LibraryReference,
            3 => CellType::MerkleProof,
            4 => CellType::MerkleUpdate,
            0xff => CellType::Ordinary,
            _ => fail!("unknown cell type {}", num),
        };
        Ok(typ)
    }
}

impl From<CellType> for u8 {
    fn from(ct: CellType) -> u8 {
        match ct {
            CellType::Unknown => 0,
            CellType::Ordinary => 0xff,
            CellType::PrunedBranch => 1,
            CellType::LibraryReference => 2,
            CellType::MerkleProof => 3,
            CellType::MerkleUpdate => 4,
        }
    }
}

impl fmt::Display for CellType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let msg = match *self {
            CellType::Ordinary => "Ordinary",
            CellType::PrunedBranch => "Pruned branch",
            CellType::LibraryReference => "Library reference",
            CellType::MerkleProof => "Merkle proof",
            CellType::MerkleUpdate => "Merkle update",
            CellType::Unknown => "Unknown",
        };
        f.write_str(msg)
    }
}

pub trait CellImpl: Any + Sync + Send + 'static {
    fn data(&self) -> &[u8];
    fn raw_data(&self) -> Result<&[u8]>;
    fn bit_length(&self) -> usize;
    fn references_count(&self) -> usize;
    fn reference(&self, index: usize) -> Result<Cell>;
    fn reference_repr_hash(&self, index: usize) -> Result<UInt256> {
        Ok(self.reference(index)?.hash(MAX_LEVEL))
    }
    fn reference_repr_depth(&self, index: usize) -> Result<u16> {
        Ok(self.reference(index)?.depth(MAX_LEVEL))
    }
    fn cell_type(&self) -> CellType;
    fn level_mask(&self) -> LevelMask;
    fn hash(&self, index: usize) -> UInt256;
    fn depth(&self, index: usize) -> u16;
    fn store_hashes(&self) -> bool;

    fn level(&self) -> u8 {
        self.level_mask().level()
    }

    fn is_merkle(&self) -> bool {
        self.cell_type() == CellType::MerkleProof || self.cell_type() == CellType::MerkleUpdate
    }

    fn is_pruned(&self) -> bool {
        self.cell_type() == CellType::PrunedBranch
    }

    fn virtualization(&self) -> u8 {
        0
    }

    fn reference_without_usage(&self, index: usize) -> Result<Cell> {
        self.reference(index)
    }
}

pub struct Cell(Arc<dyn CellImpl>);

pub(crate) static CELL_DEFAULT: LazyLock<Cell> = LazyLock::new(|| Cell(Arc::new(DataCell::new())));
#[cfg(feature = "cell_counter")]
static CELL_COUNT: LazyLock<Arc<AtomicU64>> = LazyLock::new(|| Arc::new(AtomicU64::new(0)));
// static ref FINALIZATION_NANOS: LazyLock<Arc<AtomicU64>> = LazyLock::new(Arc::new(AtomicU64::new(0)));

impl Clone for Cell {
    fn clone(&self) -> Self {
        Cell::with_cell_impl_arc(self.0.clone())
    }
}

#[cfg(feature = "cell_counter")]
impl Drop for Cell {
    fn drop(&mut self) {
        CELL_COUNT.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Cell {
    pub fn virtualize(self, offset: u8) -> Self {
        if self.level_mask().mask() == 0 {
            self
        } else {
            Cell::with_cell_impl(VirtualCell::with_cell_and_offset(self, offset))
        }
    }

    pub fn virtualization(&self) -> u8 {
        self.0.virtualization()
    }

    pub fn with_cell_impl<T: 'static + CellImpl>(cell_impl: T) -> Self {
        let ret = Cell(Arc::new(cell_impl));
        #[cfg(feature = "cell_counter")]
        CELL_COUNT.fetch_add(1, Ordering::Relaxed);
        ret
    }

    pub fn with_cell_impl_arc(cell_impl: Arc<dyn CellImpl>) -> Self {
        let ret = Cell(cell_impl);
        #[cfg(feature = "cell_counter")]
        CELL_COUNT.fetch_add(1, Ordering::Relaxed);
        ret
    }

    pub fn cell_count() -> u64 {
        #[cfg(feature = "cell_counter")]
        {
            CELL_COUNT.load(Ordering::Relaxed)
        }
        #[cfg(not(feature = "cell_counter"))]
        {
            0
        }
    }

    pub fn cell_impl(&self) -> &Arc<dyn CellImpl> {
        &self.0
    }

    // pub fn finalization_nanos() -> u64 {
    //     FINALIZATION_NANOS.load(Ordering::Relaxed)
    // }

    pub fn reference(&self, index: usize) -> Result<Cell> {
        self.0.reference(index)
    }

    pub fn reference_without_usage(&self, index: usize) -> Result<Cell> {
        self.0.reference_without_usage(index)
    }

    pub fn reference_repr_hash(&self, index: usize) -> Result<UInt256> {
        self.0.reference_repr_hash(index)
    }

    // TODO: make as simple clone
    pub fn clone_references(&self) -> SmallVec<[Cell; 4]> {
        let count = self.0.references_count();
        let mut refs = SmallVec::with_capacity(count);
        for i in 0..count {
            refs.push(self.0.reference(i).unwrap())
        }
        refs
    }

    pub fn data(&self) -> &[u8] {
        self.0.data()
    }

    fn raw_data(&self) -> Result<&[u8]> {
        self.0.raw_data()
    }

    pub fn bit_length(&self) -> usize {
        self.0.bit_length()
    }

    pub fn cell_type(&self) -> CellType {
        self.0.cell_type()
    }

    pub fn level(&self) -> u8 {
        self.0.level()
    }

    pub fn hashes_count(&self) -> usize {
        self.0.level() as usize + 1
    }

    pub fn count_cells(&self, max: usize) -> Result<usize> {
        let mut count = 0;
        let mut queue = vec![self.clone()];
        while let Some(cell) = queue.pop() {
            if count >= max {
                fail!("count exceeds max {}", max)
            }
            count += 1;
            let count = cell.references_count();
            for i in 0..count {
                queue.push(cell.reference(i)?);
            }
        }
        Ok(count)
    }

    pub fn level_mask(&self) -> LevelMask {
        self.0.level_mask()
    }

    pub fn references_count(&self) -> usize {
        self.0.references_count()
    }

    /// Returns cell's higher hash for given index (last one - representation hash)
    pub fn hash(&self, index: usize) -> UInt256 {
        self.0.hash(index)
    }

    /// Returns cell's depth for given index
    pub fn depth(&self, index: usize) -> u16 {
        self.0.depth(index)
    }

    /// Returns cell's hashes (representation and highers)
    pub fn hashes(&self) -> Vec<UInt256> {
        let mut hashes = Vec::new();
        let mut i = 0;
        while hashes.len() < self.level() as usize + 1 {
            if self.level_mask().is_significant_index(i) {
                hashes.push(self.hash(i))
            }
            i += 1;
        }
        hashes
    }

    /// Returns cell's depth (for current state and each level)
    pub fn depths(&self) -> Vec<u16> {
        let mut depths = Vec::new();
        let mut i = 0;
        while depths.len() < self.level() as usize + 1 {
            if self.level_mask().is_significant_index(i) {
                depths.push(self.depth(i))
            }
            i += 1;
        }
        depths
    }

    pub fn repr_hash(&self) -> UInt256 {
        self.0.hash(MAX_LEVEL)
    }

    pub fn repr_depth(&self) -> u16 {
        self.0.depth(MAX_LEVEL)
    }

    pub fn store_hashes(&self) -> bool {
        self.0.store_hashes()
    }

    #[allow(dead_code)]
    pub fn is_merkle(&self) -> bool {
        self.0.is_merkle()
    }

    #[allow(dead_code)]
    pub fn is_pruned(&self) -> bool {
        self.0.is_pruned()
    }

    pub fn as_library_cell(&self) -> Self {
        let mut builder =
            BuilderData::with_raw(vec![CellType::LibraryReference.into()], 8).unwrap();
        builder.append_raw(self.repr_hash().as_slice(), 256).unwrap();
        builder.set_type(CellType::LibraryReference);
        builder.into_cell().unwrap()
    }

    pub fn to_hex_string(&self, lower: bool) -> String {
        let bit_length = self.bit_length();
        if bit_length.is_multiple_of(8) {
            if lower {
                hex::encode(self.data())
            } else {
                hex::encode_upper(self.data())
            }
        } else {
            to_hex_string(self.data(), self.bit_length(), lower)
        }
    }

    pub fn write_to(&self, builder: &mut BuilderData) -> Result<()> {
        builder.checked_append_reference(self.clone())?;
        Ok(())
    }

    pub fn read_from(&mut self, slice: &mut SliceData) -> Result<()> {
        *self = slice.checked_drain_reference()?;
        Ok(())
    }

    pub fn is<T: 'static + CellImpl>(&self) -> bool {
        (&*self.0 as &dyn Any).is::<T>()
    }

    fn print_indent(
        f: &mut fmt::Formatter,
        indent: &str,
        last_child: bool,
        first_line: bool,
    ) -> fmt::Result {
        let build = match (first_line, last_child) {
            (true, true) => " └─",
            (true, false) => " ├─",
            (false, true) => "   ",
            (false, false) => " │ ",
        };
        write!(f, "{}{}", indent, build)
    }

    pub fn format_without_refs(
        &self,
        f: &mut fmt::Formatter,
        indent: &str,
        last_child: bool,
        full: bool,
        root: bool,
    ) -> fmt::Result {
        if !root {
            Self::print_indent(f, indent, last_child, true)?;
        }

        if full {
            write!(f, "{}   l: {:03b}   ", self.cell_type(), self.level_mask().mask())?;
        }

        write!(f, "bits: {}", self.bit_length())?;
        write!(f, "   refs: {}", self.references_count())?;

        if self.data().len() > 100 {
            writeln!(f)?;
            if !root {
                Self::print_indent(f, indent, last_child, false)?;
            }
        } else {
            write!(f, "   ")?;
        }

        write!(f, "data: {}", self.to_hex_string(true))?;

        if full {
            writeln!(f)?;
            if !root {
                Self::print_indent(f, indent, last_child, false)?;
            }
            write!(f, "hashes:")?;
            for h in self.hashes().iter() {
                write!(f, " {:x}", h)?;
            }
            writeln!(f)?;
            if !root {
                Self::print_indent(f, indent, last_child, false)?;
            }
            write!(f, "depths:")?;
            for d in self.depths().iter() {
                write!(f, " {}", d)?;
            }
        }
        Ok(())
    }

    pub fn format_with_refs_tree(
        &self,
        f: &mut fmt::Formatter,
        mut indent: String,
        last_child: bool,
        full: bool,
        root: bool,
        remaining_depth: u16,
    ) -> std::result::Result<String, fmt::Error> {
        self.format_without_refs(f, &indent, last_child, full, root)?;
        if remaining_depth > 0 {
            if !root {
                indent.push(' ');
                indent.push(if last_child { ' ' } else { '│' });
            }
            for i in 0..self.references_count() {
                let child = self.reference(i).unwrap();
                writeln!(f)?;
                indent = child.format_with_refs_tree(
                    f,
                    indent,
                    i == self.references_count() - 1,
                    full,
                    false,
                    remaining_depth - 1,
                )?;
            }
            if !root {
                indent.pop();
                indent.pop();
            }
        }
        Ok(indent)
    }
}

impl Deref for Cell {
    type Target = dyn CellImpl;
    fn deref(&self) -> &Self::Target {
        self.0.deref()
    }
}

impl Cell {
    pub fn read_from_file(file_name: impl AsRef<std::path::Path>) -> Self {
        let mut file = std::fs::File::open(file_name.as_ref()).unwrap();
        crate::BocReader::new().read(&mut file).unwrap().withdraw_single_root().unwrap()
    }
    pub fn write_to_file(&self, file_name: impl AsRef<std::path::Path>) {
        let mut file = std::fs::File::create(file_name.as_ref()).unwrap();
        crate::BocWriter::with_root(self).unwrap().write(&mut file).unwrap();
    }
}

impl Default for Cell {
    fn default() -> Self {
        CELL_DEFAULT.clone()
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Cell) -> bool {
        self.repr_hash() == other.repr_hash()
    }
}

impl PartialEq<UInt256> for Cell {
    fn eq(&self, other_hash: &UInt256) -> bool {
        &self.repr_hash() == other_hash
    }
}

impl Eq for Cell {}

impl fmt::Debug for Cell {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:x}", self.repr_hash())
    }
}

impl fmt::Display for Cell {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.format_with_refs_tree(
            f,
            "".to_string(),
            true,
            f.alternate(),
            true,
            min(f.precision().unwrap_or(0), MAX_DEPTH as usize) as u16,
        )?;
        Ok(())
    }
}

impl fmt::LowerHex for Cell {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_hex_string(true))
    }
}

impl fmt::UpperHex for Cell {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.to_hex_string(false))
    }
}

impl fmt::Binary for Cell {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let bitlen = self.bit_length();
        if bitlen.is_multiple_of(8) {
            write!(
                f,
                "{}",
                self.data().iter().map(|x| format!("{:08b}", *x)).collect::<Vec<_>>().join("")
            )
        } else {
            let data = self.data();
            for b in &data[..data.len() - 1] {
                write!(f, "{:08b}", b)?;
            }
            for i in (8 - (bitlen % 8)..8).rev() {
                write!(f, "{:b}", (data[data.len() - 1] >> i) & 1)?;
            }
            Ok(())
        }
    }
}

/// Calculates data's length in bits with respect to completion tag
pub fn find_tag(bitsting: &[u8]) -> usize {
    let mut length = bitsting.len() * 8;
    for x in bitsting.iter().rev() {
        if *x == 0 {
            length -= 8;
        } else {
            let mut skip = 1;
            let mut mask = 1;
            while (*x & mask) == 0 {
                skip += 1;
                mask <<= 1
            }
            length -= skip;
            break;
        }
    }
    length
}

pub fn append_tag(data: &mut SmallVec<[u8; 128]>, bits: usize) {
    let shift = bits % 8;
    if shift == 0 || data.is_empty() {
        data.truncate(bits / 8);
        data.push(0x80);
    } else {
        data.truncate(1 + bits / 8);
        let mut last_byte = data.pop().unwrap();
        if shift != 7 {
            last_byte >>= 7 - shift;
        }
        last_byte |= 1;
        if shift != 7 {
            last_byte <<= 7 - shift;
        }
        data.push(last_byte);
    }
}

// Cell layout:
// [D1] [D2] (hashes: 0..4 big endian u256) (depths: 0..4 big endian u16) [data: 0..128 bytes]
// first byte is so called desription byte 1:
// | level mask| store hashes| exotic| refs count|
// |      7 6 5|            4|      3|      2 1 0|
pub const LEVELMASK_D1_OFFSET: usize = 5;
pub const HASHES_D1_FLAG: u8 = 16;
pub const EXOTIC_D1_FLAG: u8 = 8;
pub const REFS_D1_MASK: u8 = 7;
// next byte is desription byte 2 contains data size (in special encoding, see cell_data_len)

#[inline(always)]
pub fn calc_d1(
    level_mask: LevelMask,
    store_hashes: bool,
    cell_type: CellType,
    refs_count: usize,
) -> u8 {
    (level_mask.mask() << LEVELMASK_D1_OFFSET)
        | (store_hashes as u8 * HASHES_D1_FLAG)
        | ((cell_type != CellType::Ordinary) as u8 * EXOTIC_D1_FLAG)
        | refs_count as u8
}

#[inline(always)]
pub fn calc_d2(data_bit_len: usize) -> u8 {
    ((data_bit_len / 8) << 1) as u8 + !data_bit_len.is_multiple_of(8) as u8
}

// A lot of helper-functions which incapsulates cell's layout.
// All this functions (except returning Result) can panic in case of going out of slice bounds.
#[inline(always)]
pub fn level(buf: &[u8]) -> u8 {
    level_mask(buf).level()
}

#[inline(always)]
pub fn level_mask(buf: &[u8]) -> LevelMask {
    debug_assert!(!buf.is_empty());
    LevelMask::with_mask(buf[0] >> LEVELMASK_D1_OFFSET)
}

#[inline(always)]
pub fn store_hashes(buf: &[u8]) -> bool {
    debug_assert!(!buf.is_empty());
    (buf[0] & HASHES_D1_FLAG) == HASHES_D1_FLAG
}

#[inline(always)]
pub fn exotic(buf: &[u8]) -> bool {
    debug_assert!(!buf.is_empty());
    (buf[0] & EXOTIC_D1_FLAG) == EXOTIC_D1_FLAG
}

#[inline(always)]
pub fn cell_type(buf: &[u8]) -> CellType {
    // exotic?
    if !exotic(buf) {
        // no
        CellType::Ordinary
    } else {
        match cell_data(buf).first() {
            Some(byte) => CellType::try_from(*byte).unwrap_or(CellType::Unknown),
            None => {
                debug_assert!(false, "empty exotic cell data");
                CellType::Unknown
            }
        }
    }
}

#[inline(always)]
pub fn refs_count(buf: &[u8]) -> usize {
    debug_assert!(!buf.is_empty());
    (buf[0] & REFS_D1_MASK) as usize
}

#[inline(always)]
pub fn cell_data_len(buf: &[u8]) -> usize {
    debug_assert!(buf.len() >= 2);
    ((buf[1] >> 1) + (buf[1] & 1)) as usize
}

#[inline(always)]
pub fn bit_len(buf: &[u8]) -> usize {
    debug_assert!(buf.len() >= 2);
    if buf[1] & 1 == 0 {
        (buf[1] >> 1) as usize * 8
    } else {
        find_tag(cell_data(buf))
    }
}

#[inline(always)]
pub fn data_offset(buf: &[u8]) -> usize {
    2 + (store_hashes(buf) as usize) * hashes_count(buf) * (SHA256_SIZE + DEPTH_SIZE)
}

#[inline(always)]
pub fn cell_data(buf: &[u8]) -> &[u8] {
    let data_offset = data_offset(buf);
    let cell_data_len = cell_data_len(buf);
    debug_assert!(buf.len() >= data_offset + cell_data_len);
    &buf[data_offset..data_offset + cell_data_len]
}

#[inline(always)]
pub fn hashes_count(buf: &[u8]) -> usize {
    // Hashes count depends on cell's type and level
    // - for pruned branch it's always 1
    // - for other types it's level + 1
    // To get cell type we need to calculate data's offset, but we can't do it without hashes_count.
    // So we will recognise pruned branch cell by some indirect signs - 0 refs and level != 0

    if exotic(buf) && refs_count(buf) == 0 && level(buf) != 0 {
        // pruned branch
        1
    } else {
        level(buf) as usize + 1
    }
}

#[inline(always)]
pub fn full_len(buf: &[u8]) -> usize {
    data_offset(buf) + cell_data_len(buf)
}

#[inline(always)]
pub fn hashes_len(buf: &[u8]) -> usize {
    hashes_count(buf) * SHA256_SIZE
}

#[allow(dead_code)]
#[inline(always)]
pub fn hashes(buf: &[u8]) -> &[u8] {
    debug_assert!(store_hashes(buf));
    let hashes_len = hashes_len(buf);
    debug_assert!(buf.len() >= 2 + hashes_len);
    &buf[2..2 + hashes_len]
}

#[inline(always)]
pub fn hash(buf: &[u8], index: usize) -> &[u8] {
    debug_assert!(store_hashes(buf));
    let offset = 2 + index * SHA256_SIZE;
    debug_assert!(buf.len() >= offset + SHA256_SIZE);
    &buf[offset..offset + SHA256_SIZE]
}

#[inline(always)]
pub fn depths_offset(buf: &[u8]) -> usize {
    2 + hashes_len(buf)
}

#[allow(dead_code)]
#[inline(always)]
pub fn depths_len(buf: &[u8]) -> usize {
    hashes_count(buf) * DEPTH_SIZE
}

#[allow(dead_code)]
#[inline(always)]
pub fn depths(buf: &[u8]) -> &[u8] {
    debug_assert!(store_hashes(buf));
    let offset = depths_offset(buf);
    let depths_len = depths_len(buf);
    debug_assert!(buf.len() >= offset + depths_len);
    &buf[offset..offset + depths_len]
}

#[inline(always)]
pub fn depth(buf: &[u8], index: usize) -> u16 {
    debug_assert!(store_hashes(buf));
    let offset = depths_offset(buf) + index * DEPTH_SIZE;
    let d = &buf[offset..offset + DEPTH_SIZE];
    ((d[0] as u16) << 8) | (d[1] as u16)
}

fn build_cell_buf(
    cell_type: CellType,
    data: &[u8], // with completion tag
    level_mask: u8,
    refs: usize,
) -> Result<Vec<u8>> {
    if cell_type != CellType::Ordinary && data.len() == 1 {
        fail!("Exotic cell can't have empty data");
    }
    if data.len() > MAX_DATA_BYTES {
        fail!("Cell's data can't has {} length", data.len());
    }
    if refs > MAX_REFERENCES_COUNT {
        fail!("Cell can't has {} refs", refs);
    }
    if level_mask > MAX_LEVEL_MASK {
        fail!("Level mask can't be {}", level_mask);
    }

    let data_bit_len = find_tag(data);
    let data_len = (data_bit_len / 8) + !data_bit_len.is_multiple_of(8) as usize;
    let level_mask = LevelMask::with_mask(level_mask);
    let full_length = 2 + data_len;

    debug_assert!(refs <= MAX_REFERENCES_COUNT);
    debug_assert!(data.len() <= MAX_DATA_BYTES);
    debug_assert!(level_mask.mask() <= MAX_LEVEL_MASK);
    debug_assert!(data.len() >= data_len);

    let mut buf = vec![0; full_length];
    buf[0] = calc_d1(level_mask, false, cell_type, refs);
    buf[1] = calc_d2(data_bit_len);
    let offset = 2;
    buf[offset..offset + data_len].copy_from_slice(&data[..data_len]);
    Ok(buf)
}

#[inline(always)]
fn set_hash(buf: &mut [u8], index: usize, hash: &[u8]) {
    debug_assert!(index <= level(buf) as usize);
    debug_assert!(hash.len() == SHA256_SIZE);
    let offset = 2 + index * SHA256_SIZE;
    debug_assert!(buf.len() >= offset + SHA256_SIZE);
    buf[offset..offset + SHA256_SIZE].copy_from_slice(hash);
}

#[inline(always)]
fn set_depth(buf: &mut [u8], index: usize, depth: u16) {
    debug_assert!(index <= level(buf) as usize);
    let offset = depths_offset(buf) + index * DEPTH_SIZE;
    debug_assert!(buf.len() >= offset + DEPTH_SIZE);
    buf[offset] = (depth >> 8) as u8;
    buf[offset + 1] = (depth & 0xff) as u8;
}

fn check_cell_buf(buf: &[u8], unbounded: bool) -> Result<usize> {
    if buf.len() < 2 {
        fail!("Buffer is too small to read description bytes")
    }
    let refs_count = refs_count(buf);
    if refs_count > MAX_REFERENCES_COUNT {
        fail!("Too big references count: {}", refs_count);
    }

    let full_data_len = full_len(buf);
    if buf.len() < full_data_len {
        fail!("Buffer is too small ({}) to fit cell ({})", buf.len(), full_data_len);
    }
    if !unbounded && buf.len() > full_data_len {
        log::warn!("Buffer is too big ({}), needed only {} to fit cell", buf.len(), full_data_len);
    }

    let cell_data = cell_data(buf);
    if exotic(buf) && cell_data.is_empty() {
        fail!("exotic cells must have non zero data length")
    }
    let data_bit_len = bit_len(buf);
    let expected_len = data_bit_len / 8 + !data_bit_len.is_multiple_of(8) as usize;
    if cell_data.len() != expected_len {
        log::warn!(
            "Data len wrote in description byte 2 ({} bytes) does not correspond to real length \
            calculated by tag ({} bytes, {} bits, data: {})",
            cell_data.len(),
            expected_len,
            data_bit_len,
            hex::encode(cell_data)
        );
    }
    Ok(full_data_len)
}

#[derive(Clone, Debug, PartialEq)]
enum CellBuffer {
    Local(Vec<u8>),
    External { buf: Arc<Vec<u8>>, offset: usize },
}

impl CellBuffer {
    pub fn data(&self) -> &[u8] {
        match &self {
            CellBuffer::Local(d) => d,
            CellBuffer::External { buf, offset } => {
                &buf[*offset..*offset + full_len(&buf[*offset..])]
            }
        }
    }
    pub fn unbounded_data(&self) -> &[u8] {
        match &self {
            CellBuffer::Local(d) => d,
            CellBuffer::External { buf, offset } => &buf[*offset..],
        }
    }
    pub fn unbounded_data_mut(&mut self) -> Result<&mut [u8]> {
        match self {
            CellBuffer::Local(d) => Ok(d),
            CellBuffer::External { buf: _, offset: _ } => fail!("Can't change extarnal buffer"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellData {
    buf: CellBuffer,
    hashes_depths: Vec<(UInt256, u16)>,
}

impl Default for CellData {
    fn default() -> Self {
        Self::new()
    }
}

impl CellData {
    pub fn new() -> Self {
        Self::with_params(CellType::Ordinary, &[80], 0, 0).unwrap()
    }

    pub fn with_params(
        cell_type: CellType,
        data: &[u8], // with complition tag!
        level_mask: u8,
        refs: u8,
    ) -> Result<Self> {
        let buffer = build_cell_buf(cell_type, data, level_mask, refs as usize)?;
        #[cfg(test)]
        check_cell_buf(&buffer[..], false)?;
        let hashes_count =
            if cell_type == CellType::PrunedBranch { 1 } else { level(&buffer) as usize + 1 };
        let allocate_for_hashes = hashes_count;
        let hashes_depths = Vec::with_capacity(allocate_for_hashes);
        Ok(Self { buf: CellBuffer::Local(buffer), hashes_depths })
    }

    pub fn with_external_data(buffer: &Arc<Vec<u8>>, offset: usize) -> Result<Self> {
        check_cell_buf(&buffer[offset..], true)?;

        let allocate_for_hashes =
            (!store_hashes(&buffer[offset..])) as usize * (level(&buffer[offset..]) as usize + 1);
        Ok(Self {
            buf: CellBuffer::External { buf: buffer.clone(), offset },
            hashes_depths: Vec::with_capacity(allocate_for_hashes),
        })
    }

    pub fn with_raw_data(data: Vec<u8>) -> Result<Self> {
        check_cell_buf(&data, false)?;

        let allocate_for_hashes = (!store_hashes(&data)) as usize * (level(&data) as usize + 1);
        Ok(Self {
            buf: CellBuffer::Local(data),
            hashes_depths: Vec::with_capacity(allocate_for_hashes),
        })
    }

    pub fn with_unbounded_raw_data_slice(data: &[u8]) -> Result<Self> {
        let data_len = check_cell_buf(data, true)?;

        let allocate_for_hashes = (!store_hashes(data)) as usize * (level(data) as usize + 1);
        Ok(Self {
            buf: CellBuffer::Local(data[..data_len].to_vec()),
            hashes_depths: Vec::with_capacity(allocate_for_hashes),
        })
    }

    pub fn raw_data(&self) -> &[u8] {
        self.buf.data()
    }

    pub fn cell_type(&self) -> CellType {
        cell_type(self.buf.unbounded_data())
    }

    // Might be without tag!!!
    pub fn data(&self) -> &[u8] {
        cell_data(self.buf.unbounded_data())
    }

    pub fn bit_length(&self) -> usize {
        bit_len(self.buf.unbounded_data())
    }

    pub fn level(&self) -> u8 {
        level(self.buf.unbounded_data())
    }

    pub fn level_mask(&self) -> LevelMask {
        level_mask(self.buf.unbounded_data())
    }

    pub fn store_hashes(&self) -> bool {
        store_hashes(self.buf.unbounded_data())
    }

    pub fn references_count(&self) -> usize {
        refs_count(self.buf.unbounded_data())
    }

    pub fn set_hash_depth(&mut self, index: usize, hash: &[u8], depth: u16) -> Result<()> {
        if self.store_hashes() {
            set_hash(self.buf.unbounded_data_mut()?, index, hash);
            set_depth(self.buf.unbounded_data_mut()?, index, depth);
        } else {
            debug_assert!(self.hashes_depths.len() == index);
            self.hashes_depths.push((hash.into(), depth));
        }
        Ok(())
    }

    pub fn hash(&self, index: usize) -> UInt256 {
        self.raw_hash(index).into()
    }

    pub fn raw_hash(&self, mut index: usize) -> &[u8] {
        index = self.level_mask().calc_hash_index(index);
        if self.cell_type() == CellType::PrunedBranch {
            // pruned cell stores all hashes (except representation) in data
            if index != self.level() as usize {
                let offset = 1 + 1 + index * SHA256_SIZE;
                return &self.data()[offset..offset + SHA256_SIZE];
            } else {
                index = 0;
            }
        }
        if self.store_hashes() {
            hash(self.buf.unbounded_data(), index)
        } else {
            self.hashes_depths[index].0.as_slice()
        }
    }

    pub fn depth(&self, mut index: usize) -> u16 {
        index = self.level_mask().calc_hash_index(index);
        if self.cell_type() == CellType::PrunedBranch {
            // pruned cell stores all depth except "representetion" in data
            if index != self.level() as usize {
                // type + level_mask + level * (hashes + depths)
                let offset = 1 + 1 + (self.level() as usize) * SHA256_SIZE + index * DEPTH_SIZE;
                let data = self.data();
                return ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
            } else {
                index = 0;
            }
        }
        if self.store_hashes() {
            depth(self.buf.unbounded_data(), index)
        } else {
            self.hashes_depths[index].1
        }
    }
}

#[derive(Clone, Debug)]
pub struct DataCell {
    cell_data: CellData,
    references: Vec<Cell>, // TODO make array - you already know cells refs count, or may be vector
}

impl Default for DataCell {
    fn default() -> Self {
        Self::new()
    }
}

impl DataCell {
    pub fn new() -> Self {
        Self::with_refs_and_data(vec![], &[0x80]).unwrap()
    }

    pub fn with_refs_and_data(
        references: Vec<Cell>,
        data: &[u8], // with completion tag
    ) -> Result<DataCell> {
        Self::with_params(references, data, CellType::Ordinary, 0, None)
    }

    pub fn with_params(
        references: Vec<Cell>,
        data: &[u8], // with completion tag
        cell_type: CellType,
        level_mask: u8,
        max_depth: Option<u16>,
    ) -> Result<DataCell> {
        let cell_data = CellData::with_params(cell_type, data, level_mask, references.len() as u8)?;
        Self::with_cell_data(cell_data, references, max_depth)
    }

    pub fn with_external_data(
        references: Vec<Cell>,
        buffer: &Arc<Vec<u8>>,
        offset: usize,
        max_depth: Option<u16>,
    ) -> Result<DataCell> {
        let cell_data = CellData::with_external_data(buffer, offset)?;
        Self::with_cell_data(cell_data, references, max_depth)
    }

    pub fn with_raw_data(
        references: Vec<Cell>,
        data: Vec<u8>,
        max_depth: Option<u16>,
    ) -> Result<DataCell> {
        let cell_data = CellData::with_raw_data(data)?;
        Self::with_cell_data(cell_data, references, max_depth)
    }

    pub fn with_cell_data(
        cell_data: CellData,
        references: Vec<Cell>,
        max_depth: Option<u16>,
    ) -> Result<DataCell> {
        let mut cell = DataCell { cell_data, references };
        cell.finalize(true, max_depth)?;
        Ok(cell)
    }

    #[cfg(test)]
    pub fn with_cell_data_unchecked(
        cell_data: CellData,
        references: Vec<Cell>,
    ) -> Result<DataCell> {
        Ok(DataCell { cell_data, references })
    }

    fn finalize(&mut self, force: bool, max_depth: Option<u16>) -> Result<()> {
        if !force && self.store_hashes() {
            return Ok(());
        }

        //let now = std::time::Instant::now();

        // Check data size and references count

        let bit_len = self.bit_length();
        let cell_type = self.cell_type();
        let store_hashes = self.store_hashes();

        // println!("{} {}bits {:03b}", self.cell_type(), bit_len, self.level_mask().mask());

        check_cell_layout(&self.cell_data)?;

        // Check level

        let mut children_mask = LevelMask::with_mask(0);
        for child in self.references.iter() {
            children_mask |= child.level_mask();
        }
        let level_mask = match cell_type {
            CellType::Ordinary => children_mask,
            CellType::PrunedBranch => self.level_mask(),
            CellType::LibraryReference => LevelMask::with_mask(0),
            CellType::MerkleProof => LevelMask::for_merkle_cell(children_mask),
            CellType::MerkleUpdate => LevelMask::for_merkle_cell(children_mask),
            CellType::Unknown => fail!(ExceptionCode::RangeCheckError),
        };
        if self.cell_data.level_mask() != level_mask {
            fail!(
                "Level mask mismatch {} != {}, type: {}",
                self.cell_data.level_mask(),
                level_mask,
                cell_type
            );
        }

        // calculate hashes and depths

        let is_merkle_cell = self.is_merkle();
        let is_pruned_cell = self.is_pruned();

        let mut d1d2: [u8; 2] = self.raw_data()?[..2].try_into()?;
        let max_depth = max_depth.unwrap_or(MAX_DEPTH);

        // Hashes are calculated started from smallest indexes.
        // Representation hash is calculated last and "includes" all previous hashes
        // For pruned branch cell only representation hash is calculated
        let mut hash_array_index = 0;
        for i in 0..=3 {
            // Hash is calculated only for "1" bits of level mask.
            // Hash for i = 0 is calculated anyway.
            // For example if mask = 0b010 i = 0, 2
            // for example if mask = 0b001 i = 0, 1
            // for example if mask = 0b011 i = 0, 1, 2
            if i != 0 && (is_pruned_cell || ((1 << (i - 1)) & level_mask.mask()) == 0) {
                continue;
            }

            let mut hasher = Sha256::new();
            let mut depth = 0;

            // descr bytes
            let level_mask =
                if is_pruned_cell { self.level_mask() } else { LevelMask::with_level(i as u8) };
            // "store_hashes" flag is always false while hash calculation
            d1d2[0] = calc_d1(level_mask, false, cell_type, self.references.len());
            hasher.update(d1d2);

            // data
            if i == 0 {
                let data_size = (bit_len / 8) + usize::from(!bit_len.is_multiple_of(8));
                hasher.update(&self.data()[..data_size]);
            } else {
                hasher.update(self.cell_data.raw_hash(i - 1));
            }

            // depth
            for child in self.references.iter() {
                let child_depth = child.depth(i + is_merkle_cell as usize);
                depth = max(depth, child_depth + 1);
                if depth > max_depth {
                    fail!("fail creating cell: depth {} > {}", depth, max_depth.min(MAX_DEPTH))
                }
                hasher.update(child_depth.to_be_bytes());
            }

            // hashes
            for child in self.references.iter() {
                let child_hash = child.hash(i + is_merkle_cell as usize);
                hasher.update(child_hash.as_slice());
            }

            let hash = hasher.finalize();
            if store_hashes {
                let stored_depth = self.cell_data.depth(i);
                if depth != stored_depth {
                    fail!(
                        "Calculated depth is not equal stored one ({} != {})",
                        depth,
                        stored_depth
                    );
                }
                let stored_hash = self.cell_data.raw_hash(i);
                if hash.as_slice() != stored_hash {
                    fail!("Calculated hash is not equal stored one");
                }
            } else {
                self.cell_data.set_hash_depth(hash_array_index, hash.as_slice(), depth)?;
                hash_array_index += 1;
            }
        }

        //FINALIZATION_NANOS.fetch_add(now.elapsed().as_nanos() as u64, Ordering::Relaxed);

        Ok(())
    }
}

fn check_cell_layout(cell_data: &CellData) -> Result<()> {
    let bit_len = cell_data.bit_length();
    let cell_type = cell_data.cell_type();
    let store_hashes = cell_data.store_hashes();
    let level = cell_data.level();
    let refs_count = cell_data.references_count();
    let data = cell_data.data();

    match cell_type {
        CellType::PrunedBranch => {
            // type + level_mask + level * (hashes + depths)
            let expected = 8 * (1 + 1 + (level as usize) * (SHA256_SIZE + DEPTH_SIZE));
            if bit_len != expected {
                fail!("fail creating pruned branch cell: {} != {}", bit_len, expected)
            }
            if refs_count != 0 {
                fail!("fail creating pruned branch cell: references {} != 0", refs_count)
            }
            if data[0] != u8::from(CellType::PrunedBranch) {
                fail!(
                    "fail creating pruned branch cell: data[0] {} != {}",
                    data[0],
                    u8::from(CellType::PrunedBranch)
                )
            }
            if data[1] != cell_data.level_mask().0 {
                fail!(
                    "fail creating pruned branch cell: data[1] {} != {}",
                    data[1],
                    cell_data.level_mask().0
                )
            }
            if level == 0 {
                fail!("Pruned branch cell must have non zero level");
            }
            let mut offset = 1 + 1 + (level as usize) * SHA256_SIZE;
            for _ in 0..level {
                let depth = ((data[offset] as u16) << 8) | (data[offset + 1] as u16);
                if depth > MAX_DEPTH {
                    fail!("Depth of pruned branch cell is too big");
                }
                offset += DEPTH_SIZE;
            }
            if store_hashes {
                fail!("store_hashes flag is not supported for pruned branch cell");
            }
        }
        CellType::MerkleProof => {
            // type + hash + depth
            if bit_len != 8 * (1 + SHA256_SIZE + 2) {
                fail!(
                    "fail creating merkle proof cell: bit_len {} != {}",
                    bit_len,
                    8 * (1 + SHA256_SIZE + 2)
                )
            }
            if refs_count != 1 {
                fail!("fail creating merkle proof cell: references {} != 1", refs_count)
            }
        }
        CellType::MerkleUpdate => {
            // type + 2 * (hash + depth)
            if bit_len != 8 * (1 + 2 * (SHA256_SIZE + 2)) {
                fail!(
                    "fail creating merkle unpdate cell: bit_len {} != {}",
                    bit_len,
                    8 * (1 + 2 * (SHA256_SIZE + 2))
                )
            }
            if refs_count != 2 {
                fail!("fail creating merkle unpdate cell: references {} != 2", refs_count)
            }
        }
        CellType::Ordinary => {
            if bit_len > MAX_DATA_BITS {
                fail!("fail creating ordinary cell: bit_len {} > {}", bit_len, MAX_DATA_BITS)
            }
            if refs_count > MAX_REFERENCES_COUNT {
                fail!(
                    "fail creating ordinary cell: references {} > {}",
                    refs_count,
                    MAX_REFERENCES_COUNT
                )
            }
        }
        CellType::LibraryReference => {
            if bit_len != 8 * (1 + SHA256_SIZE) {
                fail!(
                    "fail creating libray reference cell: bit_len {} != {}",
                    bit_len,
                    8 * (1 + SHA256_SIZE)
                )
            }
            if refs_count != 0 {
                fail!("fail creating libray reference cell: references {} != 0", refs_count)
            }
        }
        CellType::Unknown => {
            fail!("fail creating unknown cell")
        }
    }

    Ok(())
}

pub fn finalize_simple_cell_data(
    cell_data: &mut CellData,
    references: &[(UInt256, u16)],
    max_depth: Option<u16>,
) -> Result<()> {
    if cell_data.level() != 0 {
        fail!("Cell with nonzero level is not suitable for finalize_simple_cell");
    }

    check_cell_layout(cell_data)?;

    let mut hasher = Sha256::new();
    let mut depth = 0;

    let d1d2 = [
        calc_d1(cell_data.level_mask(), false, cell_data.cell_type(), cell_data.references_count()),
        cell_data.raw_data()[1],
    ];
    let max_depth = max_depth.unwrap_or(MAX_DEPTH);

    // descr bytes
    hasher.update(d1d2);

    // data
    let bit_len = cell_data.bit_length();
    let data_size = (bit_len / 8) + usize::from(!bit_len.is_multiple_of(8));
    hasher.update(&cell_data.data()[..data_size]);

    // depth
    for (_hash, child_depth) in references {
        depth = max(depth, *child_depth + 1);
        hasher.update(child_depth.to_be_bytes());
        if depth > max_depth {
            fail!("fail creating cell: depth {} > {}", depth, max_depth.min(MAX_DEPTH))
        }
    }

    // hashes
    for (hash, _depth) in references {
        hasher.update(hash.as_slice());
    }

    let hash = hasher.finalize();

    if cell_data.store_hashes() {
        let stored_depth = cell_data.depth(0);
        if depth != stored_depth {
            fail!("Calculated depth is not equal stored one ({} != {})", depth, stored_depth);
        }
        let stored_hash = cell_data.raw_hash(0);
        if hash.as_slice() != stored_hash {
            fail!("Calculated hash is not equal stored one");
        }
    } else {
        cell_data.set_hash_depth(0, hash.as_slice(), depth)?;
    }

    Ok(())
}

impl CellImpl for DataCell {
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
        self.references.len()
    }

    fn reference(&self, index: usize) -> Result<Cell> {
        self.references.get(index).cloned().ok_or_else(|| error!(ExceptionCode::CellUnderflow))
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

#[derive(Clone)]
struct UsageCell {
    cell: Cell,
    visit_on_load: bool,
    visited: Weak<lockfree::map::Map<UInt256, Cell>>,
}

impl UsageCell {
    fn new(
        inner: Cell,
        visit_on_load: bool,
        visited: Weak<lockfree::map::Map<UInt256, Cell>>,
    ) -> Self {
        let cell = Self { cell: inner, visit_on_load, visited };
        if visit_on_load {
            cell.visit();
        }
        cell
    }
    fn visit(&self) -> bool {
        if let Some(visited) = self.visited.upgrade() {
            visited.insert(self.cell.repr_hash(), self.cell.clone());
            return true;
        }
        false
    }
}

impl CellImpl for UsageCell {
    fn data(&self) -> &[u8] {
        if !self.visit_on_load {
            self.visit();
        }
        self.cell.data()
    }

    fn raw_data(&self) -> Result<&[u8]> {
        if !self.visit_on_load {
            self.visit();
        }
        self.cell.raw_data()
    }

    fn bit_length(&self) -> usize {
        self.cell.bit_length()
    }

    fn references_count(&self) -> usize {
        self.cell.references_count()
    }

    fn reference(&self, index: usize) -> Result<Cell> {
        if self.visit_on_load && self.visited.upgrade().is_some() || self.visit() {
            let cell = UsageCell::new(
                self.cell.reference(index)?,
                self.visit_on_load,
                self.visited.clone(),
            );
            Ok(Cell::with_cell_impl(cell))
        } else {
            self.cell.reference(index)
        }
    }

    fn cell_type(&self) -> CellType {
        self.cell.cell_type()
    }

    fn level_mask(&self) -> LevelMask {
        self.cell.level_mask()
    }

    fn hash(&self, index: usize) -> UInt256 {
        self.cell.hash(index)
    }

    fn depth(&self, index: usize) -> u16 {
        self.cell.depth(index)
    }

    fn store_hashes(&self) -> bool {
        self.cell.store_hashes()
    }

    fn reference_without_usage(&self, index: usize) -> Result<Cell> {
        self.cell.reference(index)
    }
}

#[derive(Clone)]
pub struct VirtualCell {
    offset: u8,
    cell: Cell,
}

impl VirtualCell {
    pub fn with_cell_and_offset(cell: Cell, offset: u8) -> Self {
        VirtualCell { offset, cell }
    }
}

impl CellImpl for VirtualCell {
    fn data(&self) -> &[u8] {
        self.cell.data()
    }

    fn raw_data(&self) -> Result<&[u8]> {
        fail!("Virtual cell doesn't support raw_data()");
    }

    fn bit_length(&self) -> usize {
        self.cell.bit_length()
    }

    fn references_count(&self) -> usize {
        self.cell.references_count()
    }

    fn reference(&self, index: usize) -> Result<Cell> {
        Ok(self.cell.reference(index)?.virtualize(self.offset))
    }

    fn cell_type(&self) -> CellType {
        self.cell.cell_type()
    }

    fn level_mask(&self) -> LevelMask {
        self.cell.level_mask().virtualize(self.offset)
    }

    fn hash(&self, index: usize) -> UInt256 {
        self.cell.hash(self.level_mask().calc_virtual_hash_index(index, self.offset))
    }

    fn depth(&self, index: usize) -> u16 {
        self.cell.depth(self.level_mask().calc_virtual_hash_index(index, self.offset))
    }

    fn store_hashes(&self) -> bool {
        self.cell.store_hashes()
    }

    fn virtualization(&self) -> u8 {
        self.offset
    }
}

#[derive(Debug, Default, Clone)]
pub struct UsageTree {
    root: Cell,
    original_root: Cell,
    visited: Arc<lockfree::map::Map<UInt256, Cell>>,
}

impl UsageTree {
    pub fn with_root(original_root: Cell) -> Self {
        let visited = Arc::new(lockfree::map::Map::new());
        let usage_cell = UsageCell::new(original_root.clone(), false, Arc::downgrade(&visited));
        let root = Cell::with_cell_impl_arc(Arc::new(usage_cell));
        Self { root, original_root, visited }
    }

    pub fn with_params(original_root: Cell, visit_on_load: bool) -> Self {
        let visited = Arc::new(lockfree::map::Map::new());
        let root = Cell::with_cell_impl_arc(Arc::new(UsageCell::new(
            original_root.clone(),
            visit_on_load,
            Arc::downgrade(&visited),
        )));
        Self { root, original_root, visited }
    }

    pub fn use_cell(&self, cell: Cell, visit_on_load: bool) -> Cell {
        let usage_cell = UsageCell::new(cell, visit_on_load, Arc::downgrade(&self.visited));
        usage_cell.visit();
        Cell::with_cell_impl(usage_cell)
    }

    pub fn root_cell(&self) -> Cell {
        self.root.clone()
    }

    pub fn contains(&self, hash: &UInt256) -> bool {
        self.visited.get(hash).is_some()
    }

    pub fn build_visited_subtree(
        &self,
        is_include: &impl Fn(&UInt256) -> bool,
    ) -> Result<HashSet<UInt256>> {
        let mut subvisited = HashSet::new();
        for guard in self.visited.iter() {
            if is_include(guard.key()) {
                self.visit_subtree(guard.val(), &mut subvisited)?
            }
        }
        Ok(subvisited)
    }

    fn visit_subtree(&self, cell: &Cell, subvisited: &mut HashSet<UInt256>) -> Result<()> {
        if subvisited.insert(cell.repr_hash()) {
            for i in 0..cell.references_count() {
                let child_hash = cell.reference_repr_hash(i)?;
                if let Some(guard) = self.visited.get(&child_hash) {
                    self.visit_subtree(guard.val(), subvisited)?
                }
            }
        }
        Ok(())
    }

    pub fn build_visited_set(&self) -> HashSet<UInt256> {
        let mut visited = HashSet::new();
        for guard in self.visited.iter() {
            visited.insert(guard.key().clone());
        }
        visited
    }

    pub fn original_root(&self) -> Cell {
        self.original_root.clone()
    }

    pub fn original_cell(&self, hash: &UInt256) -> Option<Cell> {
        self.visited.get(hash).map(|guard| guard.val().clone())
    }
}

// estimations
impl UsageTree {
    const PRUNNED_SIZE: usize = 41;

    pub fn estimate_proof_serialized_size(&self) -> Result<usize> {
        if self.visited.get(&self.original_root().repr_hash()).is_some() {
            self.estimate_cell(&self.original_root())
        } else {
            Ok(0)
        }
    }

    fn estimate_cell(&self, cell: &Cell) -> Result<usize> {
        let mut size = Self::estimate_serialized_size(cell);
        for i in 0..cell.references_count() {
            if let Some(child) = self.visited.get(&cell.reference_repr_hash(i)?) {
                size += self.estimate_cell(child.val())?;
            } else {
                size += Self::PRUNNED_SIZE;
            }
        }
        Ok(size)
    }

    fn estimate_serialized_size(cell: &Cell) -> usize {
        cell.bit_length().div_ceil(8) + 2 + cell.references_count() * 3 + 3
    }

    fn add_branch_internal(
        cell: &Cell,
        visited: &mut HashSet<UInt256>,
        is_include: &impl Fn(&UInt256) -> bool,
    ) -> Result<isize> {
        let mut size = (Self::PRUNNED_SIZE * cell.references_count()) as isize;
        for i in 0..cell.references_count() {
            size +=
                Self::add_branch_internal(&cell.reference_without_usage(i)?, visited, is_include)?;
        }
        if !is_include(&cell.repr_hash()) {
            Ok(0)
        } else {
            if visited.insert(cell.repr_hash()) {
                size += Self::estimate_serialized_size(cell) as isize - Self::PRUNNED_SIZE as isize;
            }
            Ok(size)
        }
    }

    pub fn add_branch_to_visited(
        cell: &Cell,
        visited: &mut HashSet<UInt256>,
        is_include: &impl Fn(&UInt256) -> bool,
    ) -> Result<usize> {
        Ok(std::cmp::max(Self::add_branch_internal(cell, visited, is_include)?, 0) as usize)
    }
}

mod slice;

pub use self::slice::*;

pub mod builder;

pub use self::builder::*;

mod builder_operations;

pub use self::builder_operations::*;
use smallvec::SmallVec;

pub(crate) fn to_hex_string(data: impl AsRef<[u8]>, len: usize, lower: bool) -> String {
    if len == 0 {
        return String::new();
    }
    let mut result = if lower { hex::encode(data) } else { hex::encode_upper(data) };
    match len % 8 {
        0 => {
            result.pop();
            result.pop();
        }
        1..=3 => {
            result.pop();
            result.push('_')
        }
        4 => {
            result.pop();
        }
        _ => result.push('_'),
    }
    result
}

pub fn create_cell(
    references: Vec<Cell>,
    data: &[u8], // with completion tag
) -> Result<Cell> {
    Ok(Cell::with_cell_impl(DataCell::with_refs_and_data(references, data)?))
}

#[cfg(test)]
#[path = "tests/test_cell.rs"]
mod tests;
