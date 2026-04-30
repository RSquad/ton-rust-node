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
use crate::{fail, ExceptionCode, Result, Sha256, UInt256};
#[cfg(feature = "cell_counter")]
use std::sync::atomic::AtomicU64;
use std::{
    alloc::Layout,
    cmp::{max, min},
    collections::HashSet,
    fmt::{self, Display, Formatter},
    hash,
    io::{Read, Write},
    ops::{BitOr, BitOrAssign},
    sync::{
        atomic::{AtomicPtr, AtomicUsize, Ordering},
        Arc, LazyLock, Mutex, Weak,
    },
};

mod slice;
pub use self::slice::*;

pub mod builder;
pub use self::builder::*;

mod builder_operations;
pub use self::builder_operations::*;

pub const SHA256_SIZE: usize = 32;
pub const DEPTH_SIZE: usize = 2;
pub const MAX_REFERENCES_COUNT: usize = 4;
pub const MAX_HASHES_COUNT: usize = 4;
pub const MAX_DATA_BITS: usize = 1023;
pub const MAX_DATA_BYTES: usize = 128; // including tag
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
    fn from(cell_type: CellType) -> u8 {
        match cell_type {
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

/// Calculates data's length in bits with respect to completion tag
pub fn find_tag(bitsting: &[u8]) -> usize {
    let mut length = bitsting.len() * 8;
    for x in bitsting.iter().rev() {
        if *x == 0 {
            length -= 8;
        } else {
            length -= 1 + x.trailing_zeros() as usize;
            break;
        }
    }
    length
}

pub fn append_tag(data: &mut smallvec::SmallVec<[u8; 128]>, bits: usize) {
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
const LEVELMASK_D1_OFFSET: usize = 5;
const HASHES_D1_FLAG: u8 = 16;
const EXOTIC_D1_FLAG: u8 = 8;
const REFS_D1_MASK: u8 = 7;
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

#[allow(dead_code)]
#[inline(always)]
fn set_hash(buf: &mut [u8], index: usize, hash: &[u8]) {
    debug_assert!(index <= level(buf) as usize);
    debug_assert!(hash.len() == SHA256_SIZE);
    let offset = 2 + index * SHA256_SIZE;
    debug_assert!(buf.len() >= offset + SHA256_SIZE);
    buf[offset..offset + SHA256_SIZE].copy_from_slice(hash);
}

#[allow(dead_code)]
#[inline(always)]
fn set_depth(buf: &mut [u8], index: usize, depth: u16) {
    debug_assert!(index <= level(buf) as usize);
    let offset = depths_offset(buf) + index * DEPTH_SIZE;
    debug_assert!(buf.len() >= offset + DEPTH_SIZE);
    buf[offset] = (depth >> 8) as u8;
    buf[offset + 1] = (depth & 0xff) as u8;
}

fn hash_from_pruned(buf: &[u8], index: usize) -> Option<&UInt256> {
    let level_mask = level_mask(buf);
    let lvl = level_mask.level() as usize;
    if index < lvl {
        let data = cell_data(buf);
        let off = 1 + 1 + index * SHA256_SIZE;
        Some(unsafe { &*(data.as_ptr().add(off) as *const UInt256) })
    } else {
        None
    }
}

fn depth_from_pruned(buf: &[u8], index: usize) -> Option<u16> {
    let level_mask = level_mask(buf);
    let lvl = level_mask.level() as usize;
    if index < lvl {
        let data = cell_data(buf);
        let off = 1 + 1 + lvl * SHA256_SIZE + index * DEPTH_SIZE;
        Some(((data[off] as u16) << 8) | (data[off + 1] as u16))
    } else {
        None
    }
}

/// Resolve hash by level index from a standard-layout buf (store_hashes=true).
/// Handles pruned branch cells transparently.
fn cell_hash(buf: &[u8], index: usize) -> &UInt256 {
    let level_mask = level_mask(buf);
    let mut array_index = level_mask.calc_hash_index(index);
    if cell_type(buf) == CellType::PrunedBranch {
        if let Some(h) = hash_from_pruned(buf, array_index) {
            return h;
        }
        array_index = 0;
    }
    unsafe { &*(hash(buf, array_index).as_ptr() as *const UInt256) }
}

/// Resolve depth by level index from a standard-layout buf (store_hashes=true).
/// Handles pruned branch cells transparently.
fn cell_depth(buf: &[u8], index: usize) -> u16 {
    let level_mask = level_mask(buf);
    let mut array_index = level_mask.calc_hash_index(index);
    if cell_type(buf) == CellType::PrunedBranch {
        if let Some(d) = depth_from_pruned(buf, array_index) {
            return d;
        }
        array_index = 0;
    }
    depth(buf, array_index)
}

/// Resolve hash and depth by level index in a single pass.
/// Avoids double level_mask/cell_type parsing compared to separate cell_hash + cell_depth.
fn cell_hash_depth(buf: &[u8], index: usize) -> (&UInt256, u16) {
    let lm = level_mask(buf);
    let mut array_index = lm.calc_hash_index(index);
    if cell_type(buf) == CellType::PrunedBranch {
        let lvl = lm.level() as usize;
        if array_index < lvl {
            let data = cell_data(buf);
            let h_off = 1 + 1 + array_index * SHA256_SIZE;
            let d_off = 1 + 1 + lvl * SHA256_SIZE + array_index * DEPTH_SIZE;
            let h = unsafe { &*(data.as_ptr().add(h_off) as *const UInt256) };
            let d = ((data[d_off] as u16) << 8) | (data[d_off + 1] as u16);
            return (h, d);
        }
        array_index = 0;
    }
    let h = unsafe { &*(hash(buf, array_index).as_ptr() as *const UInt256) };
    let d = depth(buf, array_index);
    (h, d)
}

/// Thread-safe bump allocator for arena-allocated cells.
///
/// Fast-path allocation is lock-free (atomic CAS on the bump pointer).
/// New chunk allocation (rare) is protected by a `Mutex`.
/// `contains()` is lock-free — a single `Acquire` load + flat array scan.
pub struct CellsArena {
    // Points to the current chunk's bump state.
    // `ChunkBump` pairs `current` and `end` for the same chunk, ensuring
    // a reader always sees a consistent (current, end) pair.
    // All `ChunkBump`s live in a pre-allocated Vec inside `ArenaInner`
    // and are never freed while the arena is alive.
    bump: AtomicPtr<ChunkBump>,

    chunk_size: usize,

    // Pre-allocated flat array of chunk ranges for lock-free `contains()`.
    // Invariant: ranges_ptr points to a Vec with capacity >= max_chunks,
    // so it never reallocates. Written only under `grow` lock.
    ranges_ptr: *const ChunkRange,
    ranges_count: AtomicUsize,
    max_chunks: usize,

    // Slow path: mutex for allocating new chunks.
    // Owns chunk memory (Vec<u8>), ranges, and bumps storage.
    grow: Mutex<ArenaInner>,
}

/// Bump state for a single chunk. `current` advances via CAS;
/// `end` is immutable once published.
#[repr(C)]
struct ChunkBump {
    current: AtomicPtr<u8>,
    end: *const u8,
}

#[repr(C)]
struct ChunkRange {
    start: *const u8,
    end: *const u8,
}

// ChunkRange/ChunkBump contain raw pointers but are only written under Mutex
// and read through Acquire/Release synchronization.
unsafe impl Send for ChunkRange {}
unsafe impl Sync for ChunkRange {}
unsafe impl Send for ChunkBump {}
unsafe impl Sync for ChunkBump {}

struct ArenaInner {
    chunks: Vec<Vec<u8>>,
    ranges: Vec<ChunkRange>,
    bumps: Vec<ChunkBump>,
}

// Safety: all mutable state is behind atomics or Mutex.
unsafe impl Send for CellsArena {}
unsafe impl Sync for CellsArena {}

impl CellsArena {
    /// Minimum chunk size: must fit the largest possible arena cell (LoadedCell
    /// with level 3, 4 refs, 128-byte data, fat-pointer loader ≈ 426 bytes).
    pub const MIN_CHUNK_SIZE: usize = 512;

    /// Create a new arena. `chunk_size` is the size of each chunk.
    /// `max_size` is the upper bound on total arena size (used to pre-allocate
    /// the chunk registry so `contains()` never needs a lock).
    ///
    /// # Panics
    /// - `chunk_size < MIN_CHUNK_SIZE` (512)
    /// - `max_size < chunk_size`
    pub fn new(chunk_size: usize, max_size: usize) -> Self {
        assert!(
            chunk_size >= Self::MIN_CHUNK_SIZE,
            "CellsArena: chunk_size ({}) < MIN_CHUNK_SIZE ({})",
            chunk_size,
            Self::MIN_CHUNK_SIZE,
        );
        assert!(
            max_size >= chunk_size,
            "CellsArena: max_size ({}) < chunk_size ({})",
            max_size,
            chunk_size,
        );
        let max_chunks = max_size / chunk_size + 2; // +2 for rounding + initial chunk

        let mut chunk = Vec::<u8>::with_capacity(chunk_size);
        let current = chunk.as_mut_ptr();
        let end = unsafe { current.add(chunk_size) };

        let mut ranges = Vec::with_capacity(max_chunks);
        ranges.push(ChunkRange { start: current, end });
        let ranges_ptr = ranges.as_ptr();

        let mut bumps = Vec::with_capacity(max_chunks);
        bumps.push(ChunkBump { current: AtomicPtr::new(current), end });
        let bump_ptr = bumps.as_ptr() as *mut ChunkBump;

        Self {
            bump: AtomicPtr::new(bump_ptr),
            chunk_size,
            ranges_ptr,
            ranges_count: AtomicUsize::new(1),
            max_chunks,
            grow: Mutex::new(ArenaInner { chunks: vec![chunk], ranges, bumps }),
        }
    }

    /// Lock-free check whether `ptr` belongs to any chunk in this arena.
    pub fn contains(&self, ptr: *const u8) -> bool {
        let count = self.ranges_count.load(Ordering::Acquire);
        let ranges = unsafe { std::slice::from_raw_parts(self.ranges_ptr, count) };
        ranges.iter().any(|r| ptr >= r.start && ptr < r.end)
    }

    /// Lock-free fast-path bump allocation. Falls back to `alloc_slow` when
    /// the current chunk is exhausted.
    fn alloc_raw(&self, layout: Layout) -> *mut u8 {
        loop {
            // Load bump pointer with Acquire to see fully initialized ChunkBump
            // data (pairs with Release store in alloc_slow).
            let bump = unsafe { &*self.bump.load(Ordering::Acquire) };
            let cur = bump.current.load(Ordering::Relaxed);
            let end = bump.end;

            let padding = cur.align_offset(layout.align());
            let new_cur = unsafe { cur.add(padding + layout.size()) };

            if new_cur > end as *mut u8 {
                return self.alloc_slow(layout);
            }

            if bump
                .current
                .compare_exchange_weak(cur, new_cur, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return unsafe { cur.add(padding) };
            }
            // CAS failed (contention) — retry
        }
    }

    /// Slow path: allocate a new chunk under mutex, then retry.
    #[cold]
    fn alloc_slow(&self, layout: Layout) -> *mut u8 {
        let mut inner = self.grow.lock().unwrap();

        // Double-check: another thread may have already grown the arena.
        {
            let bump = unsafe { &*self.bump.load(Ordering::Relaxed) };
            let cur = bump.current.load(Ordering::Relaxed);
            let padding = cur.align_offset(layout.align());
            let new_cur = unsafe { cur.add(padding + layout.size()) };
            if new_cur <= bump.end as *mut u8 {
                drop(inner);
                return self.alloc_raw(layout);
            }
        }

        // Allocate new chunk
        let size = self.chunk_size.max(layout.size() + layout.align());
        let mut chunk = Vec::<u8>::with_capacity(size);
        let chunk_ptr = chunk.as_mut_ptr();
        let chunk_end = unsafe { chunk_ptr.add(size) };

        // Register chunk range
        assert!(
            inner.ranges.len() < self.max_chunks,
            "CellsArena: exceeded max_chunks ({}), increase max_size",
            self.max_chunks
        );
        inner.ranges.push(ChunkRange { start: chunk_ptr, end: chunk_end });
        // Update ranges_count with Release so contains() readers see the new entry.
        self.ranges_count.store(inner.ranges.len(), Ordering::Release);

        inner.chunks.push(chunk);

        // Create new bump state and publish it.
        // bumps Vec is pre-allocated (capacity = max_chunks), so push won't
        // reallocate and all previous ChunkBump pointers remain valid.
        inner.bumps.push(ChunkBump { current: AtomicPtr::new(chunk_ptr), end: chunk_end });
        let new_bump = unsafe { inner.bumps.as_ptr().add(inner.bumps.len() - 1) } as *mut ChunkBump;
        // Release pairs with Acquire in alloc_raw, ensuring the new ChunkBump
        // data is visible before any thread starts CASing on it.
        self.bump.store(new_bump, Ordering::Release);

        drop(inner);
        self.alloc_raw(layout)
    }
}

/// A partially-constructed DataCell. Is used while reading BOC.
/// Allocated memory contains d1/d2/cell_data,
/// but hashes, depths, and references are not yet written.
/// The refs area (8 bytes per ref, aligned to 8) can be used to temporarily store
/// u32 reference indexes via `set_ref_index` / `ref_index`.
/// Finalize with `Cell::from_draft()` or drop to free the allocation.
/// Used while BOC construction to avoid multiple reallocations and copying.
pub struct BocCellDraft {
    tagged_pointer: usize,
}

unsafe impl Send for BocCellDraft {}
unsafe impl Sync for BocCellDraft {}

impl BocCellDraft {
    fn content_ptr(&self) -> *mut u8 {
        let tag = self.tagged_pointer & CELL_TAG_MASK;
        let raw_ptr = (self.tagged_pointer & !CELL_TAG_MASK) as *mut u8;
        let prefix = if tag & !CELL_TYPE_BIT == CELL_HEAP { 8 } else { 0 };
        unsafe { raw_ptr.add(prefix) }
    }

    fn buf(&self) -> &[u8] {
        DataCell::buf(self.content_ptr())
    }

    /// Number of references declared in d1.
    pub fn refs_count(&self) -> usize {
        refs_count(self.buf())
    }

    /// Read the u32 reference index stored at position `i` in the refs area.
    pub fn ref_index(&self, i: usize) -> u32 {
        debug_assert!(i < self.refs_count());
        let refs_ptr = DataCell::refs_ptr(self.content_ptr());
        unsafe { *(refs_ptr as *const u8).add(CELL_PTR_SIZE * i).cast::<u32>() }
    }

    /// Whether the draft's d1 has `store_hashes` set (i.e. hashes are pre-filled).
    /// This is set automatically by [`Cell::new_draft()`] when the input wire data
    /// contained stored hashes.
    pub fn has_stored_hashes(&self) -> bool {
        let d1 = unsafe { *self.content_ptr() };
        (d1 & HASHES_D1_FLAG) != 0
    }
}

impl Drop for BocCellDraft {
    fn drop(&mut self) {
        let tag = self.tagged_pointer & CELL_TAG_MASK;
        let raw_ptr = (self.tagged_pointer & !CELL_TAG_MASK) as *mut u8;
        if tag & !CELL_TYPE_BIT == CELL_HEAP {
            let content = unsafe { raw_ptr.add(8) };
            let buf = DataCell::buf(content);
            let data_len = cell_data_len(buf);
            let hash_count = hashes_count(buf);
            let refs_count = refs_count(buf);
            let size = 8 + DataCell::content_size(data_len, hash_count, refs_count);
            let layout = Layout::from_size_align(size, 8).unwrap();
            unsafe { std::alloc::dealloc(raw_ptr, layout) }
        }
        // arena: nothing to do
    }
}

/// TVM cell — tagged-pointer representation.
///
/// `Cell` is a single `usize` that combines a pointer to heap- or
/// arena-allocated memory with a 3-bit tag in the lowest bits (possible
/// because all allocations are 8-byte aligned). There is no Rust enum,
/// no vtable, and no separate type field — everything is encoded in the
/// pointer itself and the memory it points to.
///
/// # Tagged pointer
///
/// ```text
/// bit 0      variant:  0 = DataCell,  1 = tagged (Loaded/Usage/Virtual)
/// bits 2‑1   ownership model:
///              01  CELL_HEAP          heap, atomic refcount
///              10  CELL_ARENA  arena, no ownership tracking
/// bits 63‑3  pointer to the allocation start (8-byte aligned)
/// ```
///
/// # Allocation layout
///
/// Every allocation begins with an **ownership prefix**, followed by
/// variant-specific **content**:
///
/// ```text
/// Heap:    [AtomicUsize refcount: 8]  [content …]
/// Arena:                              [content …]
/// ```
///
/// # DataCell (bit 0 = 0)
///
/// Fully resolved cell with inline children. The internal buffer uses the
/// standard cell layout with `store_hashes` always set, so the free
/// functions [`level_mask()`], [`hash()`], [`depth()`], [`cell_data()`],
/// [`bit_len()`] etc. work directly on `buf()`.
///
/// ```text
/// [d1: 1] [d2: 1] [hashes: 32*hash_count] [depths: 2*hash_count] [cell_data: data_len]
/// [align 8] [refs: Cell * refs_count]
///
/// hash_count = hashes_count  (level + 1, or 1 for pruned branch)
/// data_len = cell_data_len (derived from d2)
/// refs_count = refs_count    (from d1, 0..4)
/// ```
///
/// # LoadedCell (variant tag = 1)
///
/// Lazy-loaded cell (e.g. from database). Stores its own hashes/depths
/// and the repr hashes/depths of each child, but loads children on demand
/// through a `loader` closure. The sub-buffer starting at `d1` has the
/// same standard layout as DataCell.
///
/// ```text
/// [tag: 1] [d1: 1] [d2: 1]
/// [hashes: 32*hash_count] [depths: 2*hash_count] [cell_data: data_len]
/// [ref_hashes: 32*refs_count] [ref_depths: 2*refs_count]
/// [align 8] [loader: 16  (Arc<dyn Fn> fat pointer)]
/// ```
///
/// # UsageCell (variant tag = 2)
///
/// Wraps an inner `Cell`, recording every access in a shared visited map.
/// If `visit_on_load` is `true` the cell is marked visited at
/// construction; otherwise — on first `data()` / `raw_data()` /
/// `reference()` call.
///
/// ```text
/// [tag: 1] [visit_on_load: 1] [pad: 6]
/// [cell: 8] [visited: 8]                 (24 bytes total)
/// ```
///
/// # VirtualCell (variant tag = 3)
///
/// Wraps an inner `Cell` with a level-mask bit shift (virtualization
/// offset). Cells with `level_mask == 0` are never wrapped —
/// `virtualize()` returns `self`.
///
/// ```text
/// [tag: 1] [offset: 1] [pad: 6] [cell: 8]   (16 bytes total)
/// ```
pub struct Cell {
    tagged_pointer: usize,
}

unsafe impl Send for Cell {}
unsafe impl Sync for Cell {}

pub type CellLoader = Arc<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>;
type WeakCellLoader = Weak<dyn Fn(&UInt256) -> Result<Cell> + Send + Sync>;

// Cell pointer tag constants

/// Tag mask for the 3 low bits of tagged_pointer
const CELL_TAG_MASK: usize = 0b111;
/// Bit 0: 0 = data cell, 1 = tagged variant (loaded/usage/virtual)
const CELL_TYPE_BIT: usize = 0b001;
/// Heap-allocated with atomic reference count (bits 2-1 = 01)
const CELL_HEAP: usize = 0b010;
/// Arena-allocated without ownership tracking (bits 2-1 = 10)
const CELL_ARENA: usize = 0b100;

/// Variant tags for non data cells (stored as first byte after ownership prefix)
const CELL_VARIANT_LOADED: u8 = 1;
const CELL_VARIANT_USAGE: u8 = 2;
const CELL_VARIANT_VIRTUAL: u8 = 3;

/// Size of a single Cell tagged pointer
const CELL_PTR_SIZE: usize = std::mem::size_of::<usize>();

// Compile-time check: 3 tag bits require 8-byte aligned allocations
const _: () = assert!(std::mem::align_of::<AtomicUsize>() >= 8);

/// Result of `Cell::check_data` — parsed and validated cell layout info.
pub struct CellRawInfo<'a> {
    pub d1: u8,
    pub d2: u8,
    pub data: &'a [u8],
    pub bit_len: usize,
}

// Variant view structs.
// Zero-sized helpers that operate on a raw content_ptr (pointer past ownership prefix).
// See the Cell doc comment above for layout details.

struct DataCell;
struct LoadedCell;
struct UsageCell;
struct VirtualCell;

impl DataCell {
    /// Standard cell buffer: [d1][d2][hashes][depths][cell_data] (d1 has store_hashes=true)
    fn buf<'a>(p: *const u8) -> &'a [u8] {
        unsafe { std::slice::from_raw_parts(p, full_len(std::slice::from_raw_parts(p, 2))) }
    }

    fn refs_offset(data_len: usize, hash_count: usize) -> usize {
        (2 + hash_count * (SHA256_SIZE + DEPTH_SIZE) + data_len + 7) & !7
    }

    fn content_size(data_len: usize, hash_count: usize, refs_count: usize) -> usize {
        let total_len = 2 + hash_count * (SHA256_SIZE + DEPTH_SIZE) + data_len;
        if refs_count == 0 {
            total_len
        } else {
            ((total_len + 7) & !7) + CELL_PTR_SIZE * refs_count
        }
    }

    fn refs_ptr(p: *mut u8) -> *mut Cell {
        let buf = Self::buf(p);
        let data_len = cell_data_len(buf);
        let hash_count = hashes_count(buf);
        unsafe { p.add(Self::refs_offset(data_len, hash_count)) as *mut Cell }
    }

    fn data<'a>(p: *const u8) -> &'a [u8] {
        cell_data(Self::buf(p))
    }
    fn raw_data<'a>(p: *const u8) -> &'a [u8] {
        Self::buf(p)
    }

    fn reference(p: *mut u8, index: usize) -> Result<Cell> {
        if index >= refs_count(Self::buf(p)) {
            fail!(ExceptionCode::CellUnderflow);
        }
        Ok(unsafe { (*Self::refs_ptr(p).add(index)).clone() })
    }

    unsafe fn drop_contents(p: *mut u8) {
        let refs_count = refs_count(Self::buf(p));
        let refs = Self::refs_ptr(p);
        for i in 0..refs_count {
            std::ptr::drop_in_place(refs.add(i));
        }
    }
}

impl LoadedCell {
    /// Standard cell buffer: [d1][d2][hashes][depths][cell_data], starts at p+1 (after tag byte)
    /// d1 has store_hashes=true; standard layout functions work directly on this buf.
    fn buf<'a>(p: *const u8) -> &'a [u8] {
        let base = unsafe { p.add(1) };
        unsafe { std::slice::from_raw_parts(base, full_len(std::slice::from_raw_parts(base, 2))) }
    }

    // Offsets from content_ptr (p), accounting for tag byte at offset 0.
    // ref_hashes/ref_depths come after the standard layout buf (= 1 + full_len).
    fn refs_hashes_offset(data_len: usize, hash_count: usize) -> usize {
        1 + 2 + hash_count * (SHA256_SIZE + DEPTH_SIZE) + data_len
    }
    fn refs_depths_offset(data_len: usize, hash_count: usize, refs_count: usize) -> usize {
        Self::refs_hashes_offset(data_len, hash_count) + SHA256_SIZE * refs_count
    }
    fn loader_offset(data_len: usize, hash_count: usize, refs_count: usize) -> usize {
        (Self::refs_depths_offset(data_len, hash_count, refs_count) + DEPTH_SIZE * refs_count + 7)
            & !7
    }
    fn content_size(data_len: usize, hash_count: usize, refs_count: usize) -> usize {
        // Arc<dyn Fn> and Weak<dyn Fn> are both fat pointers (16 bytes)
        const _: () =
            assert!(std::mem::size_of::<CellLoader>() == std::mem::size_of::<WeakCellLoader>());
        Self::loader_offset(data_len, hash_count, refs_count) + std::mem::size_of::<CellLoader>()
    }

    fn loader_ptr(p: *const u8) -> *const u8 {
        let buf = Self::buf(p);
        let (data_len, hash_count, refs_count) =
            (cell_data_len(buf), hashes_count(buf), refs_count(buf));
        unsafe { p.add(Self::loader_offset(data_len, hash_count, refs_count)) }
    }

    fn data<'a>(p: *const u8) -> &'a [u8] {
        cell_data(Self::buf(p))
    }
    fn raw_data<'a>(p: *const u8) -> &'a [u8] {
        Self::buf(p)
    }

    fn reference(p: *mut u8, index: usize, is_heap: bool) -> Result<Cell> {
        let buf = Self::buf(p);
        let refs_count = refs_count(buf);
        if index >= refs_count {
            fail!(ExceptionCode::CellUnderflow);
        }
        let (data_len, hash_count) = (cell_data_len(buf), hashes_count(buf));
        let rh_off = Self::refs_hashes_offset(data_len, hash_count) + SHA256_SIZE * index;
        let hash = unsafe { &*(p.add(rh_off) as *const UInt256) };
        let loader_ptr = Self::loader_ptr(p);
        if is_heap {
            let loader = unsafe { &*(loader_ptr as *const CellLoader) };
            loader(hash)
        } else {
            let weak = unsafe { &*(loader_ptr as *const WeakCellLoader) };
            if let Some(loader) = weak.upgrade() {
                loader(hash)
            } else {
                fail!("arena loader has been dropped")
            }
        }
    }

    fn reference_repr_hash<'a>(p: *const u8, index: usize) -> Result<&'a UInt256> {
        let buf = Self::buf(p);
        let refs_count = refs_count(buf);
        if index >= refs_count {
            fail!(ExceptionCode::CellUnderflow);
        }
        let (data_len, hash_count) = (cell_data_len(buf), hashes_count(buf));
        let rh_off = Self::refs_hashes_offset(data_len, hash_count) + SHA256_SIZE * index;
        Ok(unsafe { &*(p.add(rh_off) as *const UInt256) })
    }

    fn reference_repr_depth(p: *const u8, index: usize) -> Result<u16> {
        let buf = Self::buf(p);
        let refs_count = refs_count(buf);
        if index >= refs_count {
            fail!(ExceptionCode::CellUnderflow);
        }
        let (data_len, hash_count) = (cell_data_len(buf), hashes_count(buf));
        let ptr = unsafe {
            p.add(Self::refs_depths_offset(data_len, hash_count, refs_count) + DEPTH_SIZE * index)
        };
        Ok(unsafe { ((*ptr) as u16) << 8 | (*ptr.add(1)) as u16 })
    }

    unsafe fn drop_contents(p: *mut u8) {
        // Only called for heap cells, so always Arc
        std::ptr::drop_in_place(Self::loader_ptr(p) as *mut CellLoader);
    }
}

impl UsageCell {
    const CONTENT_SIZE: usize = 24;

    fn visit_on_load(p: *const u8) -> bool {
        unsafe { *p.add(1) != 0 }
    }
    fn cell_ptr(p: *mut u8) -> *mut Cell {
        unsafe { p.add(8) as *mut Cell }
    }
    fn visited_ptr(p: *mut u8) -> *mut Weak<lockfree::map::Map<UInt256, Cell>> {
        unsafe { p.add(16) as *mut Weak<lockfree::map::Map<UInt256, Cell>> }
    }
    fn inner<'a>(p: *const u8) -> &'a Cell {
        unsafe { &*(p.add(8) as *const Cell) }
    }
    fn visited<'a>(p: *const u8) -> &'a Weak<lockfree::map::Map<UInt256, Cell>> {
        unsafe { &*(p.add(16) as *const Weak<lockfree::map::Map<UInt256, Cell>>) }
    }

    fn reference(p: *mut u8, index: usize) -> Result<Cell> {
        let inner_ref = Self::inner(p).reference(index)?;
        Ok(Cell::usage(inner_ref, Self::visit_on_load(p), Self::visited(p).clone()))
    }

    unsafe fn drop_contents(p: *mut u8) {
        std::ptr::drop_in_place(Self::cell_ptr(p));
        std::ptr::drop_in_place(Self::visited_ptr(p));
    }
}

impl VirtualCell {
    const CONTENT_SIZE: usize = 16;

    fn offset(p: *const u8) -> u8 {
        unsafe { *p.add(1) }
    }
    fn cell_ptr(p: *mut u8) -> *mut Cell {
        unsafe { p.add(8) as *mut Cell }
    }
    fn inner<'a>(p: *const u8) -> &'a Cell {
        unsafe { &*(p.add(8) as *const Cell) }
    }

    fn reference(p: *mut u8, index: usize) -> Result<Cell> {
        let off = Self::offset(p);
        let inner_ref = Self::inner(p).reference(index)?;
        Ok(inner_ref.virtualize(off))
    }

    unsafe fn drop_contents(p: *mut u8) {
        std::ptr::drop_in_place(Self::cell_ptr(p));
    }
}

// Clone, Drop

impl Clone for Cell {
    fn clone(&self) -> Self {
        if self.is_heap() {
            let refcount = self.refcount_ptr();
            unsafe {
                (*refcount).fetch_add(1, Ordering::Relaxed);
            }
        }
        // arena: nothing to do
        Self { tagged_pointer: self.tagged_pointer }
    }
}

impl Drop for Cell {
    fn drop(&mut self) {
        if self.is_heap() {
            let refcount = self.refcount_ptr();
            if unsafe { (*refcount).fetch_sub(1, Ordering::Release) } == 1 {
                std::sync::atomic::fence(Ordering::Acquire);
                unsafe {
                    self.drop_variant_contents();
                }
                let layout = Layout::from_size_align(self.alloc_size(), 8).unwrap();
                unsafe {
                    std::alloc::dealloc(self.raw_ptr(), layout);
                }
            }
        }
        // arena: nothing to do
    }
}

impl hash::Hash for Cell {
    fn hash<H: hash::Hasher>(&self, state: &mut H) {
        self.repr_hash().hash(state);
    }
}

impl PartialEq for Cell {
    fn eq(&self, other: &Cell) -> bool {
        self.repr_hash() == other.repr_hash()
    }
}

impl PartialEq<UInt256> for Cell {
    fn eq(&self, other_hash: &UInt256) -> bool {
        *self.repr_hash() == *other_hash
    }
}

impl Eq for Cell {}

impl Default for Cell {
    fn default() -> Self {
        static EMPTY: LazyLock<Cell> =
            LazyLock::new(|| Cell::with_data_and_refs(&[0, 0], false, &[], None, None).unwrap());
        EMPTY.clone()
    }
}

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

// Cell impl

/// Trait for accessing children's hashes and depths during hash computation.
/// Abstracts over resolved `Cell` references (DataCell path) and repr-only
/// hashes/depths (LoadedCell path).
trait RefsHashes {
    fn hash(&self, ref_index: usize, level: usize) -> &UInt256;
    fn depth(&self, ref_index: usize, level: usize) -> u16;
    fn hash_depth(&self, ref_index: usize, level: usize) -> (&UInt256, u16) {
        (self.hash(ref_index, level), self.depth(ref_index, level))
    }
}

impl RefsHashes for [Cell] {
    fn hash(&self, ref_index: usize, level: usize) -> &UInt256 {
        self[ref_index].hash(level)
    }
    fn depth(&self, ref_index: usize, level: usize) -> u16 {
        self[ref_index].depth(level)
    }
    fn hash_depth(&self, ref_index: usize, level: usize) -> (&UInt256, u16) {
        self[ref_index].hash_depth(level)
    }
}

/// Repr-only reference info: returns repr hashes/depths regardless of requested level.
/// Valid only when `hashes_count == 1` (leaf, zero level mask, pruned branch), where
/// the only computed hash uses children's repr values.
struct ReprRefs<'a> {
    hashes: &'a [UInt256],
    depths: &'a [u16],
}

impl RefsHashes for ReprRefs<'_> {
    fn hash(&self, ref_index: usize, _level: usize) -> &UInt256 {
        &self.hashes[ref_index]
    }
    fn depth(&self, ref_index: usize, _level: usize) -> u16 {
        self.depths[ref_index]
    }
    fn hash_depth(&self, ref_index: usize, _level: usize) -> (&UInt256, u16) {
        (&self.hashes[ref_index], self.depths[ref_index])
    }
}

impl Cell {
    // Pointer helpers

    #[inline(always)]
    fn tag(&self) -> usize {
        self.tagged_pointer & CELL_TAG_MASK
    }
    #[inline(always)]
    fn raw_ptr(&self) -> *mut u8 {
        (self.tagged_pointer & !CELL_TAG_MASK) as *mut u8
    }
    #[inline(always)]
    fn is_data_cell(&self) -> bool {
        self.tag() & CELL_TYPE_BIT == 0
    }
    #[inline(always)]
    fn is_heap(&self) -> bool {
        self.tag() & !CELL_TYPE_BIT == CELL_HEAP
    }
    #[inline(always)]
    fn ownership_prefix_size(&self) -> usize {
        if self.is_heap() {
            8
        } else {
            0
        }
    }

    #[inline(always)]
    fn content_ptr(&self) -> *mut u8 {
        unsafe { self.raw_ptr().add(self.ownership_prefix_size()) }
    }

    #[inline(always)]
    fn refcount_ptr(&self) -> *mut AtomicUsize {
        self.raw_ptr() as *mut AtomicUsize
    }

    #[inline(always)]
    fn variant_tag(&self) -> u8 {
        debug_assert!(!self.is_data_cell());
        unsafe { *self.content_ptr() }
    }

    // Shared helpers

    /// Standard cell buffer [d1][d2][cell_data] for the underlying data cell.
    /// Delegates through Usage/Virtual wrappers to the actual data cell.
    fn cell_buf(&self) -> &[u8] {
        if self.is_data_cell() {
            DataCell::buf(self.content_ptr())
        } else {
            match self.variant_tag() {
                CELL_VARIANT_LOADED => LoadedCell::buf(self.content_ptr()),
                CELL_VARIANT_USAGE => UsageCell::inner(self.content_ptr()).cell_buf(),
                CELL_VARIANT_VIRTUAL => VirtualCell::inner(self.content_ptr()).cell_buf(),
                _ => unreachable!(),
            }
        }
    }

    // Allocation helpers

    fn alloc_size(&self) -> usize {
        self.ownership_prefix_size() + self.content_size()
    }

    fn content_size(&self) -> usize {
        if self.is_data_cell() {
            let buf = DataCell::buf(self.content_ptr());
            DataCell::content_size(cell_data_len(buf), hashes_count(buf), refs_count(buf))
        } else {
            match self.variant_tag() {
                CELL_VARIANT_LOADED => {
                    let buf = LoadedCell::buf(self.content_ptr());
                    LoadedCell::content_size(cell_data_len(buf), hashes_count(buf), refs_count(buf))
                }
                CELL_VARIANT_USAGE => UsageCell::CONTENT_SIZE,
                CELL_VARIANT_VIRTUAL => VirtualCell::CONTENT_SIZE,
                _ => unreachable!(),
            }
        }
    }

    fn alloc_cell(size: usize, arena: &Option<Arc<CellsArena>>) -> *mut u8 {
        let layout = Layout::from_size_align(size, 8).unwrap();
        match arena {
            None => unsafe {
                let p = std::alloc::alloc(layout);
                if p.is_null() {
                    log::error!("FATAL! Cell allocation of {} bytes failed", size);
                    std::process::exit(0xFF);
                }
                p
            },
            Some(arena) => arena.alloc_raw(layout),
        }
    }

    fn make_tag(arena: &Option<Arc<CellsArena>>, is_variant: bool) -> usize {
        let type_bit = if is_variant { CELL_TYPE_BIT } else { 0 };
        match arena {
            None => CELL_HEAP | type_bit,
            Some(_) => CELL_ARENA | type_bit,
        }
    }

    fn prefix_size_for(arena: &Option<Arc<CellsArena>>) -> usize {
        if arena.is_none() {
            8
        } else {
            0
        }
    }

    fn write_ownership_prefix(ptr: *mut u8, arena: &Option<Arc<CellsArena>>) {
        if arena.is_none() {
            unsafe {
                std::ptr::write(ptr as *mut AtomicUsize, AtomicUsize::new(1));
            }
        }
    }

    fn check_refs_belong_to_arena(arena: &Arc<CellsArena>, references: &[Cell]) -> Result<()> {
        for (i, r) in references.iter().enumerate() {
            if r.is_heap() || !arena.contains(r.raw_ptr()) {
                fail!("reference {} is not in the same arena", i);
            }
        }
        Ok(())
    }

    unsafe fn drop_variant_contents(&self) {
        let p = self.content_ptr();
        if self.is_data_cell() {
            DataCell::drop_contents(p);
        } else {
            match self.variant_tag() {
                CELL_VARIANT_LOADED => LoadedCell::drop_contents(p),
                CELL_VARIANT_USAGE => UsageCell::drop_contents(p),
                CELL_VARIANT_VIRTUAL => VirtualCell::drop_contents(p),
                _ => unreachable!(),
            }
        }
    }

    // Construction

    /// Validate raw cell data in standard layout: `[d1][d2][hashes?][depths?][cell_data]`.
    /// If `unbounded` is true, `raw` may be longer than `full_len`.
    /// Returns parsed cell info on success.
    pub fn check_data<'a>(raw: &'a [u8], unbounded: bool) -> Result<CellRawInfo<'a>> {
        if raw.len() < 2 {
            fail!("raw data too short: {} < 2", raw.len());
        }

        let level_mask = level_mask(raw);
        if !LevelMask::is_valid(level_mask.mask()) {
            fail!("invalid level mask {}", level_mask.mask());
        }

        let refs_count = refs_count(raw);
        if refs_count > MAX_REFERENCES_COUNT {
            fail!("references count {} > {}", refs_count, MAX_REFERENCES_COUNT);
        }

        let data_len = cell_data_len(raw);
        if data_len > MAX_DATA_BYTES {
            fail!("cell data length {} > {}", data_len, MAX_DATA_BYTES);
        }

        let expected_len = full_len(raw);
        if unbounded {
            if raw.len() < expected_len {
                fail!("raw data too short: {} < {}", raw.len(), expected_len);
            }
        } else if raw.len() != expected_len {
            fail!("raw data length mismatch: {} != {}", raw.len(), expected_len);
        }

        // Validate completion tag: if d2 says "not byte-aligned", data must have a tag bit
        if (raw[1] & 1) != 0 && data_len > 0 {
            let cell_data = cell_data(raw);
            if cell_data.last().map(|l| *l == 0) == Some(true) {
                fail!("invalid completion tag: zero last byte with tag bit set in d2");
            }
        }

        let bits_len = bit_len(raw);
        let lvl = level_mask.level();
        let is_exotic = exotic(raw);
        let has_store_hashes = store_hashes(raw);

        // If store_hashes, validate stored depths
        if has_store_hashes {
            let hash_count = hashes_count(raw);
            for i in 0..hash_count {
                let d = depth(raw, i);
                if d > MAX_DEPTH {
                    fail!("stored depth {} at index {} exceeds MAX_DEPTH", d, i);
                }
            }
        }

        // Cell type specific checks
        if !is_exotic {
            // Ordinary
            if bits_len > MAX_DATA_BITS {
                fail!("ordinary cell bit_len {} > {}", bits_len, MAX_DATA_BITS);
            }
        } else {
            let cell_data = cell_data(raw);
            if cell_data.is_empty() {
                fail!("exotic cell must have non-empty data");
            }
            let cell_type = CellType::try_from(cell_data[0])?;
            match cell_type {
                CellType::PrunedBranch => {
                    if refs_count != 0 {
                        fail!("pruned branch must have 0 references, got {}", refs_count);
                    }
                    if lvl == 0 {
                        fail!("pruned branch must have non-zero level");
                    }
                    let expected_bits = 8 * (1 + 1 + (lvl as usize) * (SHA256_SIZE + DEPTH_SIZE));
                    if bits_len != expected_bits {
                        fail!("pruned branch bit_len {} != expected {}", bits_len, expected_bits);
                    }
                    if cell_data[1] != level_mask.mask() {
                        fail!(
                            "pruned branch data[1] {} != level_mask {}",
                            cell_data[1],
                            level_mask.mask()
                        );
                    }
                    // Check depths stored in cell data
                    let mut off = 1 + 1 + (lvl as usize) * SHA256_SIZE;
                    for _ in 0..lvl {
                        let d = ((cell_data[off] as u16) << 8) | (cell_data[off + 1] as u16);
                        if d > MAX_DEPTH {
                            fail!("pruned branch depth {} exceeds MAX_DEPTH", d);
                        }
                        off += DEPTH_SIZE;
                    }
                }
                CellType::MerkleProof => {
                    let expected_bits = 8 * (1 + SHA256_SIZE + DEPTH_SIZE);
                    if bits_len != expected_bits {
                        fail!("merkle proof bit_len {} != expected {}", bits_len, expected_bits);
                    }
                    if refs_count != 1 {
                        fail!("merkle proof must have 1 reference, got {}", refs_count);
                    }
                }
                CellType::MerkleUpdate => {
                    let expected_bits = 8 * (1 + 2 * (SHA256_SIZE + DEPTH_SIZE));
                    if bits_len != expected_bits {
                        fail!("merkle update bit_len {} != expected {}", bits_len, expected_bits);
                    }
                    if refs_count != 2 {
                        fail!("merkle update must have 2 references, got {}", refs_count);
                    }
                }
                CellType::LibraryReference => {
                    let expected_bits = 8 * (1 + SHA256_SIZE);
                    if bits_len != expected_bits {
                        fail!(
                            "library reference bit_len {} != expected {}",
                            bits_len,
                            expected_bits
                        );
                    }
                    if refs_count != 0 {
                        fail!("library reference must have 0 references, got {}", refs_count);
                    }
                }
                _ => {
                    fail!("unknown exotic cell type byte {}", cell_data[0]);
                }
            }
        }

        Ok(CellRawInfo { d1: raw[0], d2: raw[1], data: cell_data(raw), bit_len: bits_len })
    }

    // Max raw data: d1(1) + d2(1) + hashes(32*4) + depths(2*4) + data(128) = 266
    const BUILD_DATA_MAX: usize = 268;

    /// Build serialized raw data: `[d1] [d2] [hashes? depths?] [cell_data]`
    pub fn build_data(
        data: &[u8],
        cell_type: CellType,
        level_mask: u8,
        refs_count: usize,
        hashes_depths: Option<&[([u8; 32], u16)]>,
    ) -> Result<smallvec::SmallVec<[u8; Self::BUILD_DATA_MAX]>> {
        if data.len() > MAX_DATA_BYTES {
            fail!("data len must be <= {MAX_DATA_BYTES}");
        }
        if refs_count > MAX_REFERENCES_COUNT {
            fail!("data len must be <= {MAX_REFERENCES_COUNT}")
        }
        if !LevelMask::is_valid(level_mask) {
            fail!("Invalid level mask {level_mask}");
        }
        let level_mask = LevelMask::with_mask(level_mask);
        let store_hashes = hashes_depths.is_some();
        let bit_length = find_tag(data);
        let d1 = calc_d1(level_mask, store_hashes, cell_type, refs_count);
        let d2 = calc_d2(bit_length);
        let mut buf = smallvec::SmallVec::new();
        buf.push(d1);
        buf.push(d2);
        let data_len = cell_data_len(&buf);
        if let Some(hd) = hashes_depths {
            if hashes_count(&buf) != hd.len() {
                fail!("level_mask and hashes_depths mismatch")
            }
            for (hash, _) in hd {
                buf.extend_from_slice(hash);
            }
            for (_, depth) in hd {
                buf.extend_from_slice(&depth.to_be_bytes());
            }
        }
        buf.extend_from_slice(&data[..data_len]);
        Ok(buf)
    }

    /// Write serialized cell data to `dest`, controlling hash inclusion via `with_hashes`.
    ///
    /// Internal representation always has `store_hashes=true` in d1. This method
    /// repacks the output according to the `with_hashes` flag:
    /// - `with_hashes=true`: writes the full `[d1][d2][hashes][depths][cell_data]` buffer.
    /// - `with_hashes=false`: rewrites d1 without the `store_hashes` flag and omits
    ///   hashes/depths, writing only `[d1][d2][cell_data]`.
    ///
    /// **PrunedBranch exception**: pruned branches never include `store_hashes` in their
    /// serialized form (higher-level hashes live in cell_data), so `with_hashes` is
    /// forced to `false` for them.
    pub fn write_data(&self, with_hashes: bool, dest: &mut dyn Write) -> Result<()> {
        let buf = self.cell_buf();
        // PrunedBranch cells never have store_hashes in serialized form
        let effective = with_hashes && cell_type(buf) != CellType::PrunedBranch;
        if effective {
            dest.write_all(buf)?;
        } else {
            let d1_no_sh = buf[0] & !HASHES_D1_FLAG;
            dest.write_all(&[d1_no_sh, buf[1]])?;
            dest.write_all(cell_data(buf))?;
        }
        Ok(())
    }

    /// Compute and write hashes/depths into a DataCell content buffer.
    ///
    /// `content` points to `[d1][d2][hashes_space][depths_space][cell_data]`.
    /// Cell data is always at offset `2 + hash_count * (SHA256_SIZE + DEPTH_SIZE)` regardless
    /// of the `store_hashes` flag in d1.
    ///
    /// **Verification of pre-filled hashes**: if `store_hashes` is set in d1, the
    /// hash/depth area is assumed to contain pre-filled values. Each computed hash and
    /// depth is verified against the pre-filled value before overwriting; a mismatch
    /// produces an error. This provides a single verification path for both
    /// [`with_references()`] and [`from_draft()`].
    ///
    /// After this function returns successfully, `store_hashes` is set in d1 and all
    /// standard layout functions work correctly on the buffer.
    /// `d1_ptr` points to the d1 byte: `[d1][d2][hashes_space][depths_space][cell_data]`.
    /// This is `content` for DataCell and `content + 1` for LoadedCell (after the tag byte).
    unsafe fn compute_hashes<R: RefsHashes + ?Sized>(
        d1_ptr: *mut u8,
        refs: &R,
        refs_count: usize,
        max_depth: u16,
    ) -> Result<()> {
        // Parse d1 directly — DataCell::buf() requires store_hashes=true in d1,
        // which may not be the case yet.
        let d1 = *d1_ptr;
        let d2 = *d1_ptr.add(1);
        let level_mask = LevelMask::with_mask(d1 >> LEVELMASK_D1_OFFSET);
        let is_exotic = (d1 & EXOTIC_D1_FLAG) != 0;
        let has_stored_hashes = (d1 & HASHES_D1_FLAG) != 0;

        // hashes_count does not depend on store_hashes
        let hash_count = if is_exotic && refs_count == 0 && level_mask.level() != 0 {
            1
        } else {
            level_mask.level() as usize + 1
        };
        let hd_size = hash_count * (SHA256_SIZE + DEPTH_SIZE);

        // Cell data is always placed after the hash/depth area
        let data_len = cell_data_len(&[d1, d2]);
        let cell_data_slice = std::slice::from_raw_parts(d1_ptr.add(2 + hd_size), data_len);

        let cell_type =
            if is_exotic { CellType::try_from(cell_data_slice[0])? } else { CellType::Ordinary };
        let is_merkle = cell_type == CellType::MerkleProof || cell_type == CellType::MerkleUpdate;
        let is_pruned = cell_type == CellType::PrunedBranch;
        let bit_length =
            if d2 & 1 == 0 { (d2 >> 1) as usize * 8 } else { find_tag(cell_data_slice) };

        let mut hash_index = 0usize;
        for i in 0..=3usize {
            if i != 0 && (is_pruned || ((1 << (i - 1)) & level_mask.mask()) == 0) {
                continue;
            }

            let mut hasher = Sha256::new();
            let mut cur_depth: u16 = 0;
            let cur_level_mask =
                if is_pruned { level_mask } else { LevelMask::with_level(i as u8) };
            hasher.update([calc_d1(cur_level_mask, false, cell_type, refs_count), d2]);

            if i == 0 {
                let data_size = (bit_length / 8) + usize::from(!bit_length.is_multiple_of(8));
                hasher.update(&cell_data_slice[..data_size]);
            } else {
                let prev_off = 2 + SHA256_SIZE * (hash_index - 1);
                hasher.update(std::slice::from_raw_parts(d1_ptr.add(prev_off), SHA256_SIZE));
            }

            let mut child_hashes = smallvec::SmallVec::<[&UInt256; MAX_REFERENCES_COUNT]>::new();
            for r_idx in 0..refs_count {
                let (child_hash, child_depth) = refs.hash_depth(r_idx, i + is_merkle as usize);
                cur_depth = max(cur_depth, child_depth + 1);
                if cur_depth > max_depth {
                    fail!("depth {} > {}", cur_depth, max_depth.min(MAX_DEPTH));
                }
                hasher.update(child_depth.to_be_bytes());
                child_hashes.push(child_hash);
            }
            for hash in child_hashes {
                hasher.update(hash.as_slice());
            }

            let computed = hasher.finalize();
            let h_off = 2 + SHA256_SIZE * hash_index;
            let d_off = 2 + SHA256_SIZE * hash_count + DEPTH_SIZE * hash_index;

            // Verify against pre-filled hashes if store_hashes was set in d1
            if has_stored_hashes {
                let stored_h = std::slice::from_raw_parts(d1_ptr.add(h_off), SHA256_SIZE);
                if computed.as_ref() != stored_h {
                    fail!("stored hash mismatch at index {}", hash_index);
                }
                let stored_d = ((*d1_ptr.add(d_off)) as u16) << 8 | (*d1_ptr.add(d_off + 1)) as u16;
                if cur_depth != stored_d {
                    fail!(
                        "stored depth mismatch at index {} ({} != {})",
                        hash_index,
                        cur_depth,
                        stored_d
                    );
                }
            }

            std::ptr::copy_nonoverlapping(computed.as_ptr(), d1_ptr.add(h_off), SHA256_SIZE);
            let depth_bytes = cur_depth.to_be_bytes();
            *d1_ptr.add(d_off) = depth_bytes[0];
            *d1_ptr.add(d_off + 1) = depth_bytes[1];
            hash_index += 1;
        }

        // Set store_hashes in d1 — from this point all layout functions work correctly
        *d1_ptr = d1 | HASHES_D1_FLAG;

        Ok(())
    }

    /// Write child Cell pointers into a DataCell content buffer at the refs offset.
    unsafe fn write_refs(content: *mut u8, references: &[Cell]) {
        let buf = DataCell::buf(content);
        let data_len = cell_data_len(buf);
        let hash_count = hashes_count(buf);
        let refs_off = DataCell::refs_offset(data_len, hash_count);
        for (i, r) in references.iter().enumerate() {
            std::ptr::write((content.add(refs_off) as *mut Cell).add(i), r.clone());
        }
    }

    /// Like write_refs but consumes the refs, avoiding extra clone/refcount bump.
    unsafe fn write_refs_owned(
        content: *mut u8,
        references: smallvec::SmallVec<[Cell; MAX_REFERENCES_COUNT]>,
    ) {
        let buf = DataCell::buf(content);
        let data_len = cell_data_len(buf);
        let hash_count = hashes_count(buf);
        let refs_off = DataCell::refs_offset(data_len, hash_count);
        for (i, r) in references.into_iter().enumerate() {
            std::ptr::write((content.add(refs_off) as *mut Cell).add(i), r);
        }
    }

    /// Allocate a DataCell content buffer with d1/d2/cell_data filled in.
    ///
    /// d1 is written WITHOUT `store_hashes` — the hash/depth area is allocated but
    /// uninitialized. [`compute_hashes()`] will set `store_hashes` in d1 after computing
    /// and writing hashes. This allows `compute_hashes` to detect pre-filled hashes:
    /// if the caller copies original hashes into the buffer and sets `store_hashes` in d1
    /// before calling `compute_hashes`, they will be verified against computed values.
    ///
    /// After `compute_hashes` completes, d1 has `store_hashes=true` and all layout
    /// functions (`data_offset`, `cell_data`, `full_len`, `DataCell::buf`) work correctly.
    fn alloc_data_cell(
        info: &CellRawInfo,
        refs_count: usize,
        arena: &Option<Arc<CellsArena>>,
    ) -> (*mut u8, usize) {
        let data_len = info.data.len();
        let level_mask = LevelMask::with_mask(info.d1 >> LEVELMASK_D1_OFFSET);
        let cell_type = if info.d1 & EXOTIC_D1_FLAG != 0 {
            CellType::try_from(info.data[0]).unwrap_or(CellType::Unknown)
        } else {
            CellType::Ordinary
        };
        // d1 without store_hashes; compute_hashes() will set the flag after writing hashes
        let d1 = calc_d1(level_mask, false, cell_type, refs_count);
        let d2 = info.d2;
        // hashes_count does not depend on store_hashes (only on level/exotic/refs)
        let hash_count = hashes_count(&[d1, d2]);
        let hd_size = hash_count * (SHA256_SIZE + DEPTH_SIZE);

        let prefix_size = Self::prefix_size_for(arena);
        let content_size = DataCell::content_size(data_len, hash_count, refs_count);
        let ptr = Self::alloc_cell(prefix_size + content_size, arena);
        Self::write_ownership_prefix(ptr, arena);
        let content = unsafe { ptr.add(prefix_size) };

        unsafe {
            *content = d1;
            *content.add(1) = d2;
            // cell_data is placed after the hash/depth area
            std::ptr::copy_nonoverlapping(info.data.as_ptr(), content.add(2 + hd_size), data_len);
        }

        (ptr, Self::make_tag(arena, false))
    }

    /// Create a DataCell from raw wire data and resolved references.
    ///
    /// Accepts input data with or without the `store_hashes` flag in d1:
    /// - If `store_hashes` is NOT set: hashes/depths are computed from cell data and children.
    /// - If `store_hashes` IS set: hashes/depths are recomputed and verified against the
    ///   stored values; a mismatch produces an error.
    ///
    /// The resulting DataCell always stores hashes internally (`store_hashes=true` in d1).
    pub fn with_data_and_refs(
        raw: &[u8],
        unbounded_data: bool,
        references: &[Cell],
        max_depth: Option<u16>,
        arena: Option<Arc<CellsArena>>,
    ) -> Result<Self> {
        let info = Self::check_data(raw, unbounded_data)?;

        let refs_count = references.len();
        if refs_count != crate::cell::refs_count(raw) {
            fail!(
                "references.len() {} != d1 refs_count {}",
                refs_count,
                crate::cell::refs_count(raw)
            );
        }

        if let Some(ref a) = arena {
            Self::check_refs_belong_to_arena(a, references)?;
        }

        let has_store_hashes = store_hashes(raw);
        let (ptr, tag) = Self::alloc_data_cell(&info, refs_count, &arena);
        let prefix_size = Self::prefix_size_for(&arena);
        let content = unsafe { ptr.add(prefix_size) };

        if has_store_hashes {
            // Copy original hashes/depths into the buffer and set store_hashes in d1,
            // so that compute_hashes will verify them against computed values.
            let d1 = unsafe { *content };
            let hash_count = hashes_count(&[d1, info.d2]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    raw[2..].as_ptr(),
                    content.add(2),
                    SHA256_SIZE * hash_count,
                );
                let depths_src = crate::cell::depths_offset(raw);
                std::ptr::copy_nonoverlapping(
                    raw[depths_src..].as_ptr(),
                    content.add(2 + SHA256_SIZE * hash_count),
                    DEPTH_SIZE * hash_count,
                );
                *content = d1 | HASHES_D1_FLAG;
            }
        }

        unsafe {
            // compute_hashes verifies pre-filled hashes (if store_hashes set) and
            // sets store_hashes in d1 upon completion.
            Self::compute_hashes(
                content,
                references,
                references.len(),
                max_depth.unwrap_or(MAX_DEPTH),
            )?;
            Self::write_refs(content, references);
        }

        Ok(Self { tagged_pointer: ptr as usize | tag })
    }

    /// Read a cell from a BOC stream and create a draft in one step.
    ///
    /// Reads wire data, validates via [`check_data`], allocates the cell, and
    /// reads reference indices directly into the cell's refs area — no
    /// intermediate refs buffer needed.
    ///
    /// `read_ref` reads one reference index from the stream (encoding depends on
    /// `ref_size` chosen by the caller).
    pub fn read_boc_draft<T: Read>(
        src: &mut T,
        cell_index: usize,
        cells_count: usize,
        arena: &Option<Arc<CellsArena>>,
        read_ref: &dyn Fn(&mut T, &mut [u8; 4]) -> std::io::Result<u32>,
    ) -> Result<BocCellDraft> {
        // --- Read wire data into stack buffer ---
        // Max raw cell: d1(1) + d2(1) + hashes(32*4) + depths(2*4) + data(128) = 266
        let mut buf = [0u8; 266];
        src.read_exact(&mut buf[0..2])?;

        let rc = refs_count(&buf);
        if rc > MAX_REFERENCES_COUNT {
            fail!("refs_count can't be {}", rc);
        }

        let wire_len = full_len(&buf[..2]);
        if wire_len > buf.len() {
            fail!("cell data too large: {}", wire_len);
        }
        if wire_len > 2 {
            src.read_exact(&mut buf[2..wire_len])?;
        }

        // BOC-specific: stricter tag-completion check than check_data
        if buf[1] & 1 != 0 && wire_len > 2 && (buf[wire_len - 1] & 0x7f == 0) {
            fail!("overly long tag-completed encoding");
        }

        // Validate & allocate (reuse shared helpers)
        let info = Self::check_data(&buf[..wire_len], true)?;
        let (ptr, tag) = Self::alloc_data_cell(&info, rc, arena);
        let prefix = Self::prefix_size_for(arena);
        let content = unsafe { ptr.add(prefix) };

        // Create draft early so Drop handles cleanup on error
        let draft = BocCellDraft { tagged_pointer: ptr as usize | tag };

        // Read refs directly into cell's refs area
        let refs_ptr = DataCell::refs_ptr(content);
        let mut ref_buf = [0u8; 4];
        for i in 0..rc {
            let r = read_ref(src, &mut ref_buf)?;
            if r >= cells_count as u32 || r <= cell_index as u32 {
                // draft is dropped here, freeing memory
                fail!(
                    "reference out of range, cells_count: {cells_count}, ref: {r}, \
                    refs_count {rc}, cell_index: {cell_index}"
                )
            }
            unsafe {
                *(refs_ptr as *mut u8).add(CELL_PTR_SIZE * i).cast::<u32>() = r;
            }
        }

        // Copy pre-existing hashes/depths from wire data
        if store_hashes(&buf) {
            let d1 = unsafe { *content };
            let hash_count = hashes_count(&[d1, info.d2]);
            unsafe {
                std::ptr::copy_nonoverlapping(
                    buf[2..].as_ptr(),
                    content.add(2),
                    SHA256_SIZE * hash_count,
                );
                let depths_src = depths_offset(&buf[..wire_len]);
                std::ptr::copy_nonoverlapping(
                    buf[depths_src..].as_ptr(),
                    content.add(2 + SHA256_SIZE * hash_count),
                    DEPTH_SIZE * hash_count,
                );
                *content = d1 | HASHES_D1_FLAG;
            }
        }

        Ok(draft)
    }

    /// Phase 2: Compute hashes/depths from children, write references, return completed Cell.
    /// Consumes the draft. On error, the draft's memory is freed automatically.
    ///
    /// If the original wire data passed to [`new_draft()`] had `store_hashes` set,
    /// the pre-filled hashes are verified against computed values inside
    /// `compute_hashes` (a mismatch produces an error).
    ///
    /// The resulting DataCell always stores hashes internally (`store_hashes=true` in d1).
    /// `arena` must be the same as the one passed to `new_draft()`, UB otherwise.
    pub fn from_boc_draft(
        draft: BocCellDraft,
        references: smallvec::SmallVec<[Cell; MAX_REFERENCES_COUNT]>,
        max_depth: Option<u16>,
        arena: Option<&Arc<CellsArena>>,
    ) -> Result<Cell> {
        let tag = draft.tagged_pointer & CELL_TAG_MASK;
        let raw_ptr = (draft.tagged_pointer & !CELL_TAG_MASK) as *mut u8;
        let prefix_size = if tag & !CELL_TYPE_BIT == CELL_HEAP { 8 } else { 0 };
        let content = unsafe { raw_ptr.add(prefix_size) };

        let refs_count = refs_count(DataCell::buf(content));
        if references.len() != refs_count {
            // draft is dropped here, freeing memory
            fail!("references.len() {} != refs_count {}", references.len(), refs_count);
        }

        if let Some(a) = arena {
            Self::check_refs_belong_to_arena(a, &references)?;
        }

        unsafe {
            Self::compute_hashes(
                content,
                references.as_slice(),
                references.len(),
                max_depth.unwrap_or(MAX_DEPTH),
            )?;
            Self::write_refs_owned(content, references);
        }

        let cell = Cell { tagged_pointer: draft.tagged_pointer };
        std::mem::forget(draft);
        Ok(cell)
    }

    /// Create a LoadedCell from raw wire data, children's repr hashes/depths, and a loader.
    ///
    /// Input data should normally have `store_hashes` set, since children are not fully
    /// available (only repr hashes/depths are known). However, the following exceptions
    /// allow input without `store_hashes` because all necessary hashes can be computed:
    /// - **PrunedBranch**: repr hash is computed from cell data; higher-level hashes are
    ///   stored in the cell data itself.
    /// - **Leaf cell** (0 references): all hashes are computable from cell data alone.
    /// - **Zero level mask**: only one hash (repr), computable from cell data and
    ///   children's repr hashes/depths.
    ///
    /// The resulting LoadedCell always stores hashes internally (`store_hashes=true` in d1).
    ///
    /// In case of arena, the loader is downgraded to `Weak`, so the caller must keep
    /// at least one more `Arc`.
    pub fn with_data_and_loader(
        raw: &[u8],
        unbounded_data: bool,
        ref_hashes: &[UInt256],
        ref_depths: &[u16],
        loader: &CellLoader,
        arena: Option<Arc<CellsArena>>,
    ) -> Result<Self> {
        let info = Self::check_data(raw, unbounded_data)?;

        let is_pruned = cell_type(raw) == CellType::PrunedBranch;
        let has_store_hashes = store_hashes(raw);
        let data_len = info.data.len();
        let refs_count = refs_count(raw);
        let is_leaf = refs_count == 0;
        let is_zero_level = level_mask(raw).mask() == 0;
        if !has_store_hashes && !is_pruned && !is_leaf && !is_zero_level {
            fail!(
                "with_refs_loader requires store_hashes in raw data, \
                 or one of: PrunedBranch, leaf cell (0 refs), zero level mask"
            );
        }
        if ref_hashes.len() != refs_count || ref_depths.len() != refs_count {
            fail!("ref_hashes/ref_depths length {} != refs_count {}", ref_hashes.len(), refs_count);
        }

        // For PrunedBranch without store_hashes, build internal buffer with store_hashes=true.
        // PrunedBranch hashes_count is always 1 (the repr hash).
        // The repr hash is extracted from the pruned cell data (the last hash at level).
        let level_mask = level_mask(raw);
        let cell_type = cell_type(raw);
        // d1 without store_hashes; compute_hashes or the has_store_hashes branch
        // will set the flag after writing hashes.
        let d1_internal = calc_d1(level_mask, false, cell_type, refs_count);
        let d2 = info.d2;
        // hashes_count does not depend on store_hashes
        let hash_count = hashes_count(&[d1_internal, d2]);
        let hd_size = hash_count * (SHA256_SIZE + DEPTH_SIZE);

        let prefix_size = Self::prefix_size_for(&arena);
        let content_size = LoadedCell::content_size(data_len, hash_count, refs_count);
        let ptr = Self::alloc_cell(prefix_size + content_size, &arena);
        Self::write_ownership_prefix(ptr, &arena);
        let content = unsafe { ptr.add(prefix_size) };

        unsafe {
            *content = CELL_VARIANT_LOADED;
            *content.add(1) = d1_internal;
            *content.add(2) = d2;

            // Copy cell data into LoadedCell buffer (after hash/depth area)
            std::ptr::copy_nonoverlapping(info.data.as_ptr(), content.add(3 + hd_size), data_len);

            if has_store_hashes {
                // Copy original hashes/depths from raw and set store_hashes in d1
                let hashes_start = 2usize;
                let depths_start = crate::cell::depths_offset(raw);
                std::ptr::copy_nonoverlapping(
                    raw[hashes_start..].as_ptr(),
                    content.add(3),
                    SHA256_SIZE * hash_count,
                );
                std::ptr::copy_nonoverlapping(
                    raw[depths_start..].as_ptr(),
                    content.add(3 + SHA256_SIZE * hash_count),
                    DEPTH_SIZE * hash_count,
                );
                *content.add(1) = d1_internal | HASHES_D1_FLAG;
            } else {
                // No store_hashes: compute hashes via compute_hashes.
                // d1 is at content+1 (after the LoadedCell tag byte).
                // Valid cases: PrunedBranch, leaf (0 refs), zero level mask — all hash_count == 1.
                debug_assert!(is_pruned || is_leaf || is_zero_level);
                debug_assert_eq!(hash_count, 1);
                let repr_refs = ReprRefs { hashes: ref_hashes, depths: ref_depths };
                Self::compute_hashes(content.add(1), &repr_refs, refs_count, MAX_DEPTH)?;
            }

            let rh_dst = content.add(LoadedCell::refs_hashes_offset(data_len, hash_count));
            for (i, h) in ref_hashes.iter().enumerate() {
                std::ptr::copy_nonoverlapping(
                    h.as_slice().as_ptr(),
                    rh_dst.add(SHA256_SIZE * i),
                    SHA256_SIZE,
                );
            }
            let rd_dst =
                content.add(LoadedCell::refs_depths_offset(data_len, hash_count, refs_count));
            for (i, &d) in ref_depths.iter().enumerate() {
                let depth_bytes = d.to_be_bytes();
                *rd_dst.add(DEPTH_SIZE * i) = depth_bytes[0];
                *rd_dst.add(DEPTH_SIZE * i + 1) = depth_bytes[1];
            }
            let loader_dst =
                content.add(LoadedCell::loader_offset(data_len, hash_count, refs_count));
            if arena.is_some() {
                let weak = Arc::downgrade(loader);
                std::ptr::write(loader_dst as *mut WeakCellLoader, weak);
            } else {
                std::ptr::write(loader_dst as *mut CellLoader, loader.clone());
            }
        }

        Ok(Self { tagged_pointer: ptr as usize | Self::make_tag(&arena, true) })
    }

    /// Creates a LoadedCell from an existing resolved Cell and a new
    /// loader for resolving children. Uses individual accessors (hash, depth, data,
    /// level_mask, etc.) to work correctly with virtualized cells.
    pub fn with_cell_and_loader(
        cell: Cell,
        loader: &CellLoader,
        arena: Option<Arc<CellsArena>>,
    ) -> Result<Self> {
        let level_mask = cell.level_mask();
        let cell_type = cell.cell_type();
        let refs_count = cell.references_count();
        let is_exotic = cell_type != CellType::Ordinary;
        let hash_count = if is_exotic && refs_count == 0 && level_mask.level() != 0 {
            1
        } else {
            level_mask.level() as usize + 1
        };
        let data = cell.data();
        let data_len = data.len();
        let d1 = calc_d1(level_mask, false, cell_type, refs_count);
        let d2 = calc_d2(cell.bit_length());
        let hd_size = hash_count * (SHA256_SIZE + DEPTH_SIZE);

        let prefix_size = Self::prefix_size_for(&arena);
        let content_size = LoadedCell::content_size(data_len, hash_count, refs_count);
        let ptr = Self::alloc_cell(prefix_size + content_size, &arena);
        Self::write_ownership_prefix(ptr, &arena);
        let content = unsafe { ptr.add(prefix_size) };

        unsafe {
            *content = CELL_VARIANT_LOADED;
            *content.add(1) = d1 | HASHES_D1_FLAG;
            *content.add(2) = d2;

            // Write hashes and depths
            //
            // For pruned branches, the cell data already contains all the hashes and depths
            // except the repr ones
            let mut idx = 0usize;
            let write_hd = |idx: usize, h: &UInt256, d: u16| {
                std::ptr::copy_nonoverlapping(
                    h.as_slice().as_ptr(),
                    content.add(3 + SHA256_SIZE * idx),
                    SHA256_SIZE,
                );
                let depth_bytes = d.to_be_bytes();
                *content.add(3 + SHA256_SIZE * hash_count + DEPTH_SIZE * idx) = depth_bytes[0];
                *content.add(3 + SHA256_SIZE * hash_count + DEPTH_SIZE * idx + 1) = depth_bytes[1];
            };
            if cell_type == CellType::PrunedBranch {
                write_hd(0, cell.repr_hash(), cell.repr_depth());
            } else {
                let mut i = 0;
                while idx < hash_count {
                    if level_mask.is_significant_index(i) {
                        write_hd(idx, cell.hash(i), cell.depth(i));
                        idx += 1;
                    }
                    i += 1;
                }
            }

            // Copy cell data
            std::ptr::copy_nonoverlapping(data.as_ptr(), content.add(3 + hd_size), data_len);

            // Write children's repr hashes
            let rh_dst = content.add(LoadedCell::refs_hashes_offset(data_len, hash_count));
            for i in 0..refs_count {
                let child_hash = cell.reference_repr_hash(i)?;
                std::ptr::copy_nonoverlapping(
                    child_hash.as_slice().as_ptr(),
                    rh_dst.add(SHA256_SIZE * i),
                    SHA256_SIZE,
                );
            }
            // Write children's repr depths
            let rd_dst =
                content.add(LoadedCell::refs_depths_offset(data_len, hash_count, refs_count));
            for i in 0..refs_count {
                let child_depth = cell.reference_repr_depth(i)?;
                let depth_bytes = child_depth.to_be_bytes();
                *rd_dst.add(DEPTH_SIZE * i) = depth_bytes[0];
                *rd_dst.add(DEPTH_SIZE * i + 1) = depth_bytes[1];
            }
            // Write loader (downgrade to Weak for arena cells)
            let loader_dst =
                content.add(LoadedCell::loader_offset(data_len, hash_count, refs_count));
            if arena.is_some() {
                let weak = Arc::downgrade(loader);
                std::ptr::write(loader_dst as *mut WeakCellLoader, weak);
            } else {
                std::ptr::write(loader_dst as *mut CellLoader, loader.clone());
            }
        }

        Ok(Self { tagged_pointer: ptr as usize | Self::make_tag(&arena, true) })
    }

    pub fn usage(
        cell: Cell,
        visit_on_load: bool,
        visited: Weak<lockfree::map::Map<UInt256, Cell>>,
    ) -> Self {
        let prefix_size = 8;
        let ptr = Self::alloc_cell(prefix_size + UsageCell::CONTENT_SIZE, &None);
        Self::write_ownership_prefix(ptr, &None);
        let content = unsafe { ptr.add(prefix_size) };
        unsafe {
            *content = CELL_VARIANT_USAGE;
            *content.add(1) = visit_on_load as u8;
            std::ptr::write(content.add(8) as *mut Cell, cell);
            std::ptr::write(
                content.add(16) as *mut Weak<lockfree::map::Map<UInt256, Cell>>,
                visited,
            );
        }
        let ucell = Self { tagged_pointer: ptr as usize | CELL_HEAP | CELL_TYPE_BIT };
        if visit_on_load {
            ucell.visit();
        }
        ucell
    }

    pub fn virtualize(self, offset: u8) -> Self {
        if self.level_mask().mask() == 0 {
            return self;
        }
        let prefix_size = 8;
        let ptr = Self::alloc_cell(prefix_size + VirtualCell::CONTENT_SIZE, &None);
        Self::write_ownership_prefix(ptr, &None);
        let content = unsafe { ptr.add(prefix_size) };
        unsafe {
            *content = CELL_VARIANT_VIRTUAL;
            *content.add(1) = offset;
            std::ptr::write(content.add(8) as *mut Cell, self);
        }
        Self { tagged_pointer: ptr as usize | CELL_HEAP | CELL_TYPE_BIT }
    }

    pub fn visit(&self) {
        if !self.is_data_cell() && self.variant_tag() == CELL_VARIANT_USAGE {
            let p = self.content_ptr();
            let weak = UsageCell::visited(p);
            if let Some(strong) = weak.upgrade() {
                strong.insert(self.repr_hash().clone(), UsageCell::inner(p).clone());
            }
        }
    }

    pub fn virtualization(&self) -> u8 {
        if !self.is_data_cell() {
            let v = self.variant_tag();
            if v == CELL_VARIANT_VIRTUAL {
                return VirtualCell::offset(self.content_ptr());
            } else if v == CELL_VARIANT_USAGE {
                let p = self.content_ptr();
                return UsageCell::inner(p).virtualization();
            }
        }
        0
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

    pub fn data(&self) -> &[u8] {
        if self.is_data_cell() {
            DataCell::data(self.content_ptr())
        } else {
            match self.variant_tag() {
                CELL_VARIANT_LOADED => LoadedCell::data(self.content_ptr()),
                CELL_VARIANT_USAGE => {
                    let p = self.content_ptr();
                    if !UsageCell::visit_on_load(p) {
                        self.visit();
                    }
                    UsageCell::inner(p).data()
                }
                CELL_VARIANT_VIRTUAL => VirtualCell::inner(self.content_ptr()).data(),
                _ => unreachable!(),
            }
        }
    }

    pub fn raw_data(&self) -> Result<&[u8]> {
        if self.is_data_cell() {
            Ok(DataCell::raw_data(self.content_ptr()))
        } else {
            match self.variant_tag() {
                CELL_VARIANT_LOADED => Ok(LoadedCell::raw_data(self.content_ptr())),
                CELL_VARIANT_USAGE => {
                    let p = self.content_ptr();
                    if !UsageCell::visit_on_load(p) {
                        self.visit();
                    }
                    UsageCell::inner(p).raw_data()
                }
                CELL_VARIANT_VIRTUAL => fail!("raw_data not supported for virtual cells"),
                _ => unreachable!(),
            }
        }
    }

    pub fn bit_length(&self) -> usize {
        bit_len(self.cell_buf())
    }

    pub fn cell_type(&self) -> CellType {
        cell_type(self.cell_buf())
    }

    pub fn level_mask(&self) -> LevelMask {
        if !self.is_data_cell() {
            match self.variant_tag() {
                CELL_VARIANT_USAGE => {
                    return UsageCell::inner(self.content_ptr()).level_mask();
                }
                CELL_VARIANT_VIRTUAL => {
                    let p = self.content_ptr();
                    return VirtualCell::inner(p).level_mask().virtualize(VirtualCell::offset(p));
                }
                _ => {}
            }
        }
        level_mask(self.cell_buf())
    }

    pub fn level(&self) -> u8 {
        self.level_mask().level()
    }

    pub fn hashes_count(&self) -> usize {
        if !self.is_data_cell() {
            match self.variant_tag() {
                CELL_VARIANT_USAGE => return UsageCell::inner(self.content_ptr()).hashes_count(),
                CELL_VARIANT_VIRTUAL => {
                    return if self.cell_type() == CellType::PrunedBranch {
                        1
                    } else {
                        self.level() as usize + 1
                    };
                }
                _ => {}
            }
        }
        hashes_count(self.cell_buf())
    }

    pub fn references_count(&self) -> usize {
        refs_count(self.cell_buf())
    }

    pub fn store_hashes(&self) -> bool {
        store_hashes(self.cell_buf())
    }

    pub fn hash(&self, index: usize) -> &UInt256 {
        if !self.is_data_cell() {
            match self.variant_tag() {
                CELL_VARIANT_USAGE => return UsageCell::inner(self.content_ptr()).hash(index),
                CELL_VARIANT_VIRTUAL => {
                    let p = self.content_ptr();
                    let off = VirtualCell::offset(p);
                    let inner = VirtualCell::inner(p);
                    let virt_idx = inner.level_mask().calc_virtual_hash_index(index, off);
                    return inner.hash(virt_idx);
                }
                _ => {}
            }
        }
        cell_hash(self.cell_buf(), index)
    }

    pub fn depth(&self, index: usize) -> u16 {
        if !self.is_data_cell() {
            match self.variant_tag() {
                CELL_VARIANT_USAGE => return UsageCell::inner(self.content_ptr()).depth(index),
                CELL_VARIANT_VIRTUAL => {
                    let p = self.content_ptr();
                    let off = VirtualCell::offset(p);
                    let inner = VirtualCell::inner(p);
                    let virt_idx = inner.level_mask().calc_virtual_hash_index(index, off);
                    return inner.depth(virt_idx);
                }
                _ => {}
            }
        }
        cell_depth(self.cell_buf(), index)
    }

    pub fn hash_depth(&self, index: usize) -> (&UInt256, u16) {
        if !self.is_data_cell() {
            match self.variant_tag() {
                CELL_VARIANT_USAGE => {
                    let inner = UsageCell::inner(self.content_ptr());
                    return inner.hash_depth(index);
                }
                CELL_VARIANT_VIRTUAL => {
                    let p = self.content_ptr();
                    let off = VirtualCell::offset(p);
                    let inner = VirtualCell::inner(p);
                    let virt_idx = inner.level_mask().calc_virtual_hash_index(index, off);
                    return inner.hash_depth(virt_idx);
                }
                _ => {}
            }
        }
        cell_hash_depth(self.cell_buf(), index)
    }

    pub fn reference_repr_hash(&self, index: usize) -> Result<UInt256> {
        if self.is_data_cell() {
            Ok(self.reference(index)?.repr_hash().clone())
        } else if self.variant_tag() == CELL_VARIANT_LOADED {
            Ok(LoadedCell::reference_repr_hash(self.content_ptr(), index)?.clone())
        } else {
            Ok(self.reference(index)?.repr_hash().clone())
        }
    }

    pub fn reference_repr_depth(&self, ref_index: usize) -> Result<u16> {
        if self.is_data_cell() {
            Ok(self.reference(ref_index)?.repr_depth())
        } else {
            match self.variant_tag() {
                CELL_VARIANT_LOADED => {
                    LoadedCell::reference_repr_depth(self.content_ptr(), ref_index)
                }
                _ => Ok(self.reference(ref_index)?.repr_depth()),
            }
        }
    }

    pub fn reference(&self, index: usize) -> Result<Cell> {
        if self.is_data_cell() {
            DataCell::reference(self.content_ptr(), index)
        } else {
            match self.variant_tag() {
                CELL_VARIANT_LOADED => {
                    LoadedCell::reference(self.content_ptr(), index, self.is_heap())
                }
                CELL_VARIANT_USAGE => {
                    let p = self.content_ptr();
                    if !UsageCell::visit_on_load(p) {
                        self.visit();
                    }
                    UsageCell::reference(p, index)
                }
                CELL_VARIANT_VIRTUAL => VirtualCell::reference(self.content_ptr(), index),
                _ => unreachable!(),
            }
        }
    }

    pub fn reference_without_usage(&self, index: usize) -> Result<Cell> {
        if !self.is_data_cell() && self.variant_tag() == CELL_VARIANT_USAGE {
            UsageCell::inner(self.content_ptr()).reference(index)
        } else {
            self.reference(index)
        }
    }

    pub fn clone_references(&self) -> Result<smallvec::SmallVec<[Cell; 4]>> {
        let count = self.references_count();
        let mut refs = smallvec::SmallVec::with_capacity(count);
        for i in 0..count {
            refs.push(self.reference(i)?);
        }
        Ok(refs)
    }

    pub fn repr_hash(&self) -> &UInt256 {
        self.hash(MAX_LEVEL)
    }
    pub fn repr_depth(&self) -> u16 {
        self.depth(MAX_LEVEL)
    }

    pub fn hashes(&self) -> smallvec::SmallVec<[&UInt256; MAX_HASHES_COUNT]> {
        let mut hashes = smallvec::SmallVec::new();
        let mut i = 0;
        while hashes.len() < self.level() as usize + 1 {
            if self.level_mask().is_significant_index(i) {
                hashes.push(self.hash(i));
            }
            i += 1;
        }
        hashes
    }

    pub fn depths(&self) -> smallvec::SmallVec<[u16; MAX_HASHES_COUNT]> {
        let mut depths = smallvec::SmallVec::new();
        let mut i = 0;
        while depths.len() < self.level() as usize + 1 {
            if self.level_mask().is_significant_index(i) {
                depths.push(self.depth(i));
            }
            i += 1;
        }
        depths
    }

    /// Returns a pointer to the loader function data for a loaded cell.
    /// Can be used to compare two cells' loaders via pointer equality (Arc::ptr_eq).
    /// Returns None for non-loaded cells or arena-allocated loaded cells.
    pub fn loader_data_ptr(&self) -> Option<*const ()> {
        if self.is_data_cell() || self.variant_tag() != CELL_VARIANT_LOADED || !self.is_heap() {
            return None;
        }
        let loader_ptr = LoadedCell::loader_ptr(self.content_ptr());
        let loader = unsafe { &*(loader_ptr as *const CellLoader) };
        Some(Arc::as_ptr(loader) as *const ())
    }

    #[allow(dead_code)]
    pub fn is_merkle(&self) -> bool {
        let cell_type = self.cell_type();
        cell_type == CellType::MerkleUpdate || cell_type == CellType::MerkleProof
    }

    #[allow(dead_code)]
    pub fn is_pruned(&self) -> bool {
        self.cell_type() == CellType::PrunedBranch
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

    // The function is used only for testing purposes (in VM).
    // So it is ok to panic sometimes
    pub fn as_library_cell(&self) -> Self {
        let mut builder =
            BuilderData::with_raw(vec![CellType::LibraryReference.into()], 8).unwrap();
        builder.append_raw(self.repr_hash().as_slice(), 256).unwrap();
        builder.set_type(CellType::LibraryReference);
        builder.into_cell().unwrap()
    }

    fn print_indent(
        f: &mut fmt::Formatter,
        indent: &str,
        last_child: bool,
        first_line: bool,
    ) -> fmt::Result {
        write!(
            f,
            "{}{}",
            indent,
            match (first_line, last_child) {
                (true, true) => " └─",
                (true, false) => " ├─",
                (false, true) => "   ",
                (false, false) => " │ ",
            }
        )
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
        write!(f, "bits: {}   refs: {}", self.bit_length(), self.references_count())?;
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
                writeln!(f)?;
                match self.reference(i) {
                    Ok(r) => {
                        indent = r.format_with_refs_tree(
                            f,
                            indent,
                            i == self.references_count() - 1,
                            full,
                            false,
                            remaining_depth - 1,
                        )?
                    }
                    Err(e) => {
                        write!(
                            f,
                            "error loading ref {:x}: {e}",
                            self.reference_repr_hash(i).unwrap_or_default()
                        )?;
                    }
                }
            }
            if !root {
                indent.pop();
                indent.pop();
            }
        }
        Ok(indent)
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
        Self::with_params(original_root, false)
    }

    pub fn with_params(original_root: Cell, visit_on_load: bool) -> Self {
        let visited = Arc::new(lockfree::map::Map::new());
        let root = Cell::usage(original_root.clone(), visit_on_load, Arc::downgrade(&visited));
        Self { root, original_root, visited }
    }

    pub fn use_cell(&self, cell: Cell, visit_on_load: bool) -> Cell {
        Cell::usage(cell, visit_on_load, Arc::downgrade(&self.visited))
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
        if subvisited.insert(cell.repr_hash().clone()) {
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
        if self.visited.get(self.original_root().repr_hash()).is_some() {
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
        if !is_include(cell.repr_hash()) {
            Ok(0)
        } else {
            if visited.insert(cell.repr_hash().clone()) {
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
    references: &[Cell],
    data: &[u8], // with completion tag
) -> Result<Cell> {
    Cell::with_data_and_refs(
        &Cell::build_data(data, CellType::Ordinary, 0, references.len(), None)?,
        false,
        references,
        None,
        None,
    )
}

#[cfg(test)]
#[path = "tests/test_cell.rs"]
mod tests;
