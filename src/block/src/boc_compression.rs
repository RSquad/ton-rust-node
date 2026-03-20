/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
//! # BOC Compression Module
//!
//! This module implements compression algorithms for TON Bag-of-Cells (BOC) serialization.
//! It provides two compression algorithms:
//!
//! - **BaselineLZ4** (algorithm byte = 0x00): Simple LZ4 compression of standard BOC serialization.
//! - **ImprovedStructureLZ4** (algorithm byte = 0x01): Structure-aware compression that exploits
//!   the cell DAG topology for better compression ratios.
//!
//! ## Wire Format Overview
//!
//! Both algorithms use a common outer wrapper:
//! ```text
//! +--------+---------------------------+
//! | 1 byte | Variable length           |
//! | algo   | algorithm-specific payload|
//! +--------+---------------------------+
//! ```
//!
//! ### ImprovedStructureLZ4 Inner Format
//!
//! After LZ4 decompression, the inner bitstream (MSB-first) is structured as:
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────────┐
//! │ [1] Header (4 bytes BE)        - Decompressed size                      │
//! │ [2] LZ4 Payload                - Contains the following bitstream:      │
//! │     ┌───────────────────────────────────────────────────────────────────┤
//! │     │ [3] Stream Header                                                 │
//! │     │     • root_count:     32 bits                                     │
//! │     │     • root_indexes[]: 32 bits × root_count                        │
//! │     │     • node_count:     32 bits                                     │
//! │     ├───────────────────────────────────────────────────────────────────┤
//! │     │ [4] Cell Metadata Section (per node, topological order)           │
//! │     │     • cell_type:  4 bits (0=Ordinary, 1-8=Special+level, 9=ct9)   │
//! │     │     • refs_cnt:   4 bits (0-4)                                    │
//! │     │     • [optional] length: 1 bit (small) + 7 bits (value)           │
//! │     ├───────────────────────────────────────────────────────────────────┤
//! │     │ [5] Direct Edge Bitmap (1 bit per edge)                           │
//! │     │     • 1 = child is immediate successor (rank = parent_rank + 1)   │
//! │     │     • 0 = non-direct edge (delta encoded later)                   │
//! │     ├───────────────────────────────────────────────────────────────────┤
//! │     │ [6] Prefix Bits Section (sub-byte remainders for small/pruned)    │
//! │     │     • For each small/pruned cell: (bit_length % 8) bits           │
//! │     ├───────────────────────────────────────────────────────────────────┤
//! │     │ [7] Graph Delta Section (non-direct edge encoding)                │
//! │     │     • Byte-boundary-aware three-case encoding                     │
//! │     ├───────────────────────────────────────────────────────────────────┤
//! │     │ [8] Byte Alignment Padding (0-7 bits)                             │
//! │     ├───────────────────────────────────────────────────────────────────┤
//! │     │ [9] Cell Data Section                                             │
//! │     │     • Small/Pruned: remaining bits after prefix                   │
//! │     │     • Non-small: 0*1<data> (padding + marker + data)              │
//! │     ├───────────────────────────────────────────────────────────────────┤
//! │     │ [10] Final Padding (to byte boundary)                             │
//! └─────┴───────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## C++ Reference
//!
//! This implementation is designed to be wire-compatible with the C++ implementation in:
//! `docs/ton-node-cpp/crypto/vm/boc-compression.cpp`
//!
//! Key C++ types and their Rust equivalents:
//! - `td::BitSlice` / `td::BitString` → `BitSliceReader` / `BitStringWriter`
//! - `td::RefInt256` for grams arithmetic → `i128` (sufficient for Grams ≤ 120 bits)
//! - `vm::CellBuilder::finalize(is_special)` → `set_special_cell_type_from_data()` + `into_cell()`

use crate::{
    error, fail, BocFlags, BocReader, BocWriter, BuilderData, Cell, CellType, CurrencyCollection,
    Deserializable, Grams, IBitstring, Result, Serializable, SliceData, UInt256,
};
use std::{collections::HashMap, io::Cursor, vec::Vec};

// ============================================================================
// Public API
// ============================================================================

/// Compression algorithm identifier.
///
/// The algorithm byte is prepended to the compressed payload to identify
/// the decompression method.
#[derive(PartialEq, Clone, Copy)]
#[repr(u8)]
pub enum CompressionAlgorithm {
    /// Simple LZ4 compression of standard BOC serialization.
    BaselineLZ4 = 0,
    /// Structure-aware compression exploiting cell DAG topology.
    ImprovedStructureLZ4 = 1,
}

/// Size of the decompressed length header in bytes (4 bytes, big-endian).
const K_DECOMPRESSED_SIZE: usize = 4;

/// Decompresses BOC data using the algorithm specified in the first byte.
///
/// # Arguments
/// * `compressed` - Compressed data with algorithm byte prefix
/// * `max_size` - Maximum allowed decompressed size (security limit)
///
/// # Returns
/// Vector of root cells on success
///
/// # Wire Format
/// ```text
/// +--------+---------------------------+
/// | 1 byte | Variable length           |
/// | algo   | algorithm-specific payload|
/// +--------+---------------------------+
/// ```
pub fn boc_decompress(compressed: impl AsRef<[u8]>, max_size: usize) -> Result<Vec<Cell>> {
    if max_size == 0 {
        fail!("Can't decompress empty data");
    }
    let compressed_data = compressed.as_ref()[1..].to_vec();
    match compressed.as_ref()[0] {
        0 => {
            let decompressed = boc_decompress_baseline_lz4(compressed_data, max_size)?;
            Ok(BocReader::new().read(&mut Cursor::new(&decompressed))?.roots)
        }
        1 => boc_decompress_improved_structure_lz4(compressed_data, max_size),
        any => Err(anyhow::format_err!("Invalid compression algorithm {}", any)),
    }
}

/// Compresses BOC roots using the specified algorithm.
///
/// # Arguments
/// * `boc_roots` - Vector of root cells to compress
/// * `algo` - Compression algorithm to use
///
/// # Returns
/// Compressed data with algorithm byte prefix
pub fn boc_compress(boc_roots: Vec<Cell>, algo: CompressionAlgorithm) -> Result<Vec<u8>> {
    if boc_roots.is_empty() {
        fail!("Cannot compress empty BOC roots");
    };

    let mut compressed = if algo == CompressionAlgorithm::BaselineLZ4 {
        boc_compress_baseline_lz4(boc_roots)?
    } else if algo == CompressionAlgorithm::ImprovedStructureLZ4 {
        boc_compress_improved_structure_lz4(boc_roots)?
    } else {
        fail!("Unknown compression algorithm")
    };

    compressed.insert(0, algo as u8);
    Ok(compressed)
}

// ============================================================================
// BaselineLZ4 Implementation
// ============================================================================

/// Compresses BOC using simple LZ4 compression of standard BOC serialization.
///
/// # Wire Format
/// ```text
/// +----------------+------------------+
/// | 4 bytes BE     | Variable length  |
/// | decompressed   | LZ4 payload      |
/// | size           |                  |
/// +----------------+------------------+
/// ```
pub fn boc_compress_baseline_lz4(boc_roots: Vec<Cell>) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    BocWriter::with_flags(boc_roots, BocFlags::all())?.write(&mut data)?;
    let mut compressed = lz4::block::compress(&data, None, true)?;

    // Replace LZ4's size header with big-endian decompressed size
    let size: Vec<u8> = (data.len() as u32).to_be_bytes().to_vec();
    compressed[0..4].copy_from_slice(&size);
    Ok(compressed)
}

/// Decompresses BaselineLZ4 format.
///
/// # Arguments
/// * `compressed` - Compressed data (without algorithm byte)
/// * `max_size` - Maximum allowed decompressed size
pub fn boc_decompress_baseline_lz4(compressed: Vec<u8>, max_size: usize) -> Result<Vec<u8>> {
    if compressed.len() < K_DECOMPRESSED_SIZE {
        fail!("BOC decompression failed: input too small for header");
    }
    let (size_bytes, payload) = compressed.split_at(K_DECOMPRESSED_SIZE);
    let size_bytes: [u8; 4] = size_bytes.try_into().map_err(|_| error!("Cannot convert size"))?;
    let decompressed_size = u32::from_be_bytes(size_bytes) as usize;
    if decompressed_size > max_size {
        fail!("BOC decompression failed: invalid decompressed size");
    }

    let decompressed = lz4::block::decompress(payload, Some(decompressed_size as i32))?;
    Ok(decompressed)
}

// ============================================================================
// ImprovedStructureLZ4 Implementation
// ============================================================================
//
// This section implements the structure-aware BOC compression algorithm.
// The C++ reference implementation is in:
//   `docs/ton-node-cpp/crypto/vm/boc-compression.cpp`
//
// Key design principles:
// 1. Cells are ordered by topological sort with specific tie-breaking rules
// 2. The DAG structure is encoded separately from cell data
// 3. Direct successor edges (child_rank == parent_rank + 1) are encoded as single bits
// 4. Non-direct edges use byte-boundary-aware delta encoding
// 5. MerkleUpdate cells can use "depth-balance elision" (cell_type=9) to skip
//    encoding subtree data that can be reconstructed from diffs
// ============================================================================

/// Raw cell data extracted from a `Cell` for compression.
///
/// This stores the cell's data bits separately from its structure,
/// allowing the compression algorithm to encode structure and data independently.
#[derive(Clone, Debug)]
struct CellBits {
    /// Raw byte data (may contain unused bits in the last byte)
    bytes: Vec<u8>,
    /// Exact number of valid bits
    bit_len: usize,
}

impl CellBits {
    fn new(bytes: Vec<u8>, bit_len: usize) -> Self {
        Self { bytes, bit_len }
    }
}

// ============================================================================
// Bit I/O Primitives
// ============================================================================
//
// These structures provide MSB-first bit-level I/O compatible with C++:
// - `td::BitSlice` for reading
// - `td::BitString` for writing
//
// Bit ordering: Within each byte, bit 0 is the MSB (byte >> 7), bit 7 is the LSB.
// This matches the TON convention and C++ implementation.
// ============================================================================

/// MSB-first bit reader compatible with C++ `td::BitSlice`.
///
/// # Bit Ordering
/// ```text
/// Byte:  [b7 b6 b5 b4 b3 b2 b1 b0]
///         ↑ MSB (bit 0 of byte)
/// ```
///
/// # C++ Equivalence
/// - `bs.size()` → `remaining_bits()`
/// - `orig_size - bs.size()` → `consumed_bits()`
/// - `bs.bits().get_uint(n)` → `read_uint(n)`
/// - `bs.advance(n)` → implicit in `read_uint()` or explicit `skip_bits()`
///
/// # Important
/// No implicit byte-alignment is performed. The caller must explicitly
/// align when required by the wire format (e.g., between sections).
struct BitSliceReader<'a> {
    /// Source byte slice
    data: &'a [u8],
    /// Total bits available (data.len() * 8)
    bit_len: usize,
    /// Current read position in bits (0 = start of data)
    pos: usize,
}

impl<'a> BitSliceReader<'a> {
    /// Creates a new reader over the given byte slice.
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit_len: data.len() * 8, pos: 0 }
    }

    /// Returns the number of unread bits remaining.
    /// C++ equivalent: `bs.size()`
    fn remaining_bits(&self) -> usize {
        self.bit_len.saturating_sub(self.pos)
    }

    /// Returns the number of bits consumed so far.
    /// C++ equivalent: `orig_size - bs.size()`
    fn consumed_bits(&self) -> usize {
        self.pos
    }

    /// Peeks at the next bit without consuming it.
    /// Returns `None` if no bits remain.
    fn peek_bit(&self) -> Option<bool> {
        if self.pos >= self.bit_len {
            return None;
        }
        let byte = self.data[self.pos / 8];
        let bit = (byte >> (7 - (self.pos % 8))) & 1;
        Some(bit == 1)
    }

    /// Reads an unsigned integer of up to 32 bits, MSB-first.
    /// C++ equivalent: `bs.bits().get_uint(bits)` followed by `bs.advance(bits)`
    fn read_uint(&mut self, bits: usize) -> Result<u32> {
        if bits == 0 {
            return Ok(0);
        }
        if bits > 32 {
            fail!("BOC decompression failed: too many bits to read");
        }
        if self.remaining_bits() < bits {
            fail!("BOC decompression failed: not enough bits to read");
        }
        let mut value: u32 = 0;
        for _ in 0..bits {
            let byte = self.data[self.pos / 8];
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            value = (value << 1) | (bit as u32);
            self.pos += 1;
        }
        Ok(value)
    }

    /// Skips the specified number of bits without reading.
    /// C++ equivalent: `bs.advance(bits)`
    fn skip_bits(&mut self, bits: usize) -> Result<()> {
        if self.remaining_bits() < bits {
            fail!("BOC decompression failed: not enough bits to read");
        }
        self.pos += bits;
        Ok(())
    }

    /// Reads bits directly into a `BuilderData`.
    /// C++ equivalent: `cb.store_bits(bs.subslice(0, bits))` followed by `bs.advance(bits)`
    fn read_bits_to_builder(&mut self, builder: &mut BuilderData, bits: usize) -> Result<()> {
        if bits == 0 {
            return Ok(());
        }
        if self.remaining_bits() < bits {
            fail!("BOC decompression failed: not enough bits to read");
        }

        // Fast path: byte-aligned on both sides
        if self.pos.is_multiple_of(8) && bits.is_multiple_of(8) {
            let start = self.pos / 8;
            let end = start + bits / 8;
            builder.append_raw(&self.data[start..end], bits)?;
            self.pos += bits;
            return Ok(());
        }

        // Slow path: bit-by-bit copy for unaligned access
        let mut out = vec![0u8; bits.div_ceil(8)];
        for i in 0..bits {
            let byte = self.data[self.pos / 8];
            let bit = (byte >> (7 - (self.pos % 8))) & 1;
            if bit == 1 {
                out[i / 8] |= 1 << (7 - (i % 8));
            }
            self.pos += 1;
        }
        builder.append_raw(&out, bits)?;
        Ok(())
    }
}

/// MSB-first bit writer compatible with C++ `td::BitString`.
///
/// Used to construct the inner bitstream before LZ4 compression.
///
/// # Bit Ordering
/// Same as `BitSliceReader`: MSB-first within each byte.
///
/// # C++ Equivalence
/// - `bs.reserve_bitslice(n).bits().store_uint(v, n)` → `push_uint(v, n)`
/// - `bs.append(slice)` → `push_bits()`
/// - `bs.size()` → `len_bits()`
#[derive(Default)]
struct BitStringWriter {
    /// Output byte buffer
    data: Vec<u8>,
    /// Number of bits written
    bit_len: usize,
}

impl BitStringWriter {
    /// Creates a writer with pre-allocated capacity for the specified number of bits.
    fn with_capacity_bits(bits: usize) -> Self {
        Self { data: Vec::with_capacity(bits.div_ceil(8)), bit_len: 0 }
    }

    /// Returns the number of bits written.
    /// C++ equivalent: `bs.size()`
    fn len_bits(&self) -> usize {
        self.bit_len
    }

    /// Writes a single bit.
    fn push_bit(&mut self, bit: bool) {
        let byte_pos = self.bit_len / 8;
        let bit_pos = self.bit_len % 8;
        if bit_pos == 0 {
            self.data.push(0);
        }
        if bit {
            self.data[byte_pos] |= 1 << (7 - bit_pos);
        }
        self.bit_len += 1;
    }

    /// Writes an unsigned integer as MSB-first bits.
    /// C++ equivalent: `append_uint(bs, value, bits)`
    fn push_uint(&mut self, value: u64, bits: usize) {
        if bits == 0 {
            return;
        }
        let mask = if bits >= 64 { u64::MAX } else { (1u64 << bits) - 1 };
        let v = value & mask;
        for i in (0..bits).rev() {
            self.push_bit(((v >> i) & 1) == 1);
        }
    }

    /// Writes the specified number of zero bits.
    fn push_zeros(&mut self, bits: usize) {
        for _ in 0..bits {
            self.push_bit(false);
        }
    }

    /// Copies bits from a source byte slice.
    ///
    /// # Arguments
    /// * `src` - Source byte slice
    /// * `src_bit_offset` - Starting bit position in source
    /// * `bits` - Number of bits to copy
    fn push_bits(&mut self, src: &[u8], src_bit_offset: usize, bits: usize) {
        if bits == 0 {
            return;
        }

        // Fast path: byte-aligned on both sides
        if self.bit_len.is_multiple_of(8)
            && src_bit_offset.is_multiple_of(8)
            && bits.is_multiple_of(8)
        {
            let start = src_bit_offset / 8;
            let end = start + bits / 8;
            self.data.extend_from_slice(&src[start..end]);
            self.bit_len += bits;
            return;
        }

        // Slow path: bit-by-bit copy
        for i in 0..bits {
            let pos = src_bit_offset + i;
            let byte = src[pos / 8];
            let bit = (byte >> (7 - (pos % 8))) & 1;
            self.push_bit(bit == 1);
        }
    }
}

// ============================================================================
// Graph Encoding Helpers
// ============================================================================

/// Calculates the number of bits required to encode a graph edge delta.
///
/// # Algorithm
/// The delta encoding uses variable-width integers. The bit width is determined
/// by the maximum possible delta value at position `i`:
///   `max_delta = node_count - i - 3`
///
/// The number of bits required is `ceil(log2(max_delta + 1))` = `bit_length(max_delta)`.
///
/// # C++ Reference
/// ```cpp
/// required_bits = 1 + (31 ^ td::count_leading_zeroes32(node_count - i - 3))
/// ```
/// This is equivalent to `32 - clz(x)` which equals `bit_length(x)`.
///
/// # Arguments
/// * `node_count` - Total number of nodes in the graph
/// * `i` - Current node's topological rank
///
/// # Panics
/// Caller must ensure `node_count > i + 3`.
fn required_bits_for_delta(node_count: usize, i: usize) -> usize {
    let x = (node_count - i - 3) as u32;
    (32 - x.leading_zeros()) as usize
}

// ============================================================================
// MerkleUpdate and Depth-Balance Helpers
// ============================================================================
//
// The ImprovedStructureLZ4 format supports an optimization for MerkleUpdate cells:
// "Depth-Balance Elision" (cell_type = 9).
//
// In ShardAccounts Merkle updates, each node contains a DepthBalanceInfo with
// the sum of grams in its subtree. When compressing:
// - If a right-subtree node's grams can be derived from (left_grams + sum_of_child_diffs),
//   the encoder can emit cell_type=9 and skip the payload entirely.
//
// During decompression:
// - When encountering cell_type=9, the decoder reconstructs the grams value
//   by computing left_grams + sum_of_child_diffs.
// ============================================================================

/// Detects if a partially-built cell is a MerkleUpdate based on the special flag
/// and first data byte (tag = 0x04).
///
/// # C++ Reference
/// ```cpp
/// bool is_merkle_update_node(bool is_special, const vm::CellBuilder& cb)
/// ```
///
/// # Arguments
/// * `is_special` - Whether the cell is marked as special
/// * `builder` - The cell builder containing partial cell data
fn is_merkle_update_node(is_special: bool, builder: &BuilderData) -> bool {
    if !is_special {
        return false;
    }
    if builder.length_in_bits() < 8 {
        return false;
    }
    // MerkleUpdate tag byte is 0x04
    builder.data().first().copied() == Some(0x04)
}

/// Extracts grams value from a DepthBalanceInfo cell.
///
/// # DepthBalanceInfo Format (TL-B)
/// ```text
/// depth_balance$_ split_depth:(#<= 30) balance:CurrencyCollection = DepthBalanceInfo;
/// ```
///
/// For the MerkleUpdate optimization, we only handle the simplified case:
/// - Empty HmLabel prefix ('00' = 2 bits)
/// - split_depth = 0 (5 bits)
/// - CurrencyCollection with only grams (no extra currencies)
/// - No remaining bits
///
/// # C++ Reference
/// ```cpp
/// td::RefInt256 extract_balance_from_depth_balance_info(vm::CellSlice& cs)
/// ```
///
/// # Returns
/// `Some(grams)` if the cell matches the expected format, `None` otherwise.
///
/// # Note on Integer Size
/// TON Grams is `VarUInteger 16` (up to 15 bytes = 120 bits), so `u128` is sufficient.
/// Intermediate differences fit in `i128` since we only add/subtract grams values.
fn extract_depth_balance_grams(cell: &Cell) -> Option<u128> {
    let mut cs = SliceData::load_cell_ref(cell).ok()?;

    // Check for empty HmLabel ('00' = 2 zero bits)
    if cs.remaining_bits() < 2 {
        return None;
    }
    if cs.get_next_int(2).ok()? != 0 {
        return None;
    }

    // Check split_depth = 0 (5 bits)
    if cs.remaining_bits() < 5 {
        return None;
    }
    let split_depth = cs.get_next_int(5).ok()? as u8;
    if split_depth != 0 {
        return None;
    }

    // Parse CurrencyCollection
    let balance = CurrencyCollection::construct_from(&mut cs).ok()?;

    // Must have only grams (no extra currencies)
    if !balance.other.is_empty() {
        return None;
    }

    // Must consume all bits
    if cs.remaining_bits() != 0 {
        return None;
    }

    Some(balance.grams.as_u128())
}

/// Computes the grams difference between paired left/right DepthBalanceInfo cells.
///
/// This is used during MerkleUpdate subtree reconstruction to accumulate diffs.
///
/// # C++ Reference
/// ```cpp
/// td::RefInt256 process_shard_accounts_vertex(vm::CellSlice& cs_left, vm::CellSlice& cs_right)
/// ```
///
/// # Returns
/// `Some(right_grams - left_grams)` if both cells are valid DepthBalanceInfo, `None` otherwise.
fn process_shard_accounts_vertex(left: &Cell, right: &Cell) -> Option<i128> {
    let left_grams = extract_depth_balance_grams(left)? as i128;
    let right_grams = extract_depth_balance_grams(right)? as i128;
    Some(right_grams - left_grams)
}

/// Writes a DepthBalanceInfo structure containing only grams.
///
/// This is used to reconstruct cell_type=9 (depth-balance elision) nodes
/// during decompression.
///
/// # Wire Format
/// ```text
/// +--------+-------------+----------------------+
/// | 2 bits | 5 bits      | Variable             |
/// | '00'   | split_depth | CurrencyCollection   |
/// | HmLabel| = 0         | (grams only)         |
/// +--------+-------------+----------------------+
/// ```
///
/// # C++ Reference
/// ```cpp
/// bool write_depth_balance_grams(vm::CellBuilder& cb, const td::RefInt256& grams)
/// ```
fn write_depth_balance_grams(builder: &mut BuilderData, grams: u128) -> Result<()> {
    // Empty HmLabel ('00') + split_depth=0 (5 bits) = 7 zero bits
    builder.append_bits(0, 7)?;

    // CurrencyCollection with only grams
    let grams = Grams::try_from(grams)?;
    CurrencyCollection::from_grams(grams).write_to(builder)?;
    Ok(())
}

/// Sets the cell type for special cells based on the first data byte (tag).
///
/// In C++, `CellBuilder::finalize(is_special)` automatically infers the concrete
/// special type from the first byte. In Rust, we must set `CellType` explicitly
/// before calling `into_cell()`.
///
/// # Special Cell Tags
/// | Tag  | CellType         |
/// |------|------------------|
/// | 0x01 | PrunedBranch     |
/// | 0x02 | LibraryReference |
/// | 0x03 | MerkleProof      |
/// | 0x04 | MerkleUpdate     |
///
/// # C++ Reference
/// ```cpp
/// nodes[idx] = cell_builders[idx].finalize(is_special[idx]);
/// ```
fn set_special_cell_type_from_data(builder: &mut BuilderData, is_special: bool) -> Result<()> {
    if !is_special {
        return Ok(());
    }
    if builder.length_in_bits() < 8 {
        fail!("BOC decompression failed: invalid special cell");
    }
    let tag = builder.data().first().copied().unwrap_or(0);
    let cell_type = match tag {
        0x01 => CellType::PrunedBranch,
        0x02 => CellType::LibraryReference,
        0x03 => CellType::MerkleProof,
        0x04 => CellType::MerkleUpdate,
        _ => fail!("BOC decompression failed: unknown special cell type"),
    };
    builder.set_type(cell_type);
    Ok(())
}

// ============================================================================
// ImprovedStructureLZ4 Decompression
// ============================================================================

/// Decompresses BOC data using the C++-compatible ImprovedStructureLZ4 algorithm.
///
/// This function decodes the structure-aware compression format that separates
/// cell topology from cell data for improved compression ratios.
///
/// # C++ Reference
/// ```cpp
/// td::Result<std::vector<td::Ref<vm::Cell>>>
/// boc_decompress_improved_structure_lz4(td::Slice compressed, int max_decompressed_size)
/// ```
/// File: `docs/ton-node-cpp/crypto/vm/boc-compression.cpp`
///
/// # Wire Format (after LZ4 decompression)
///
/// The inner bitstream is processed in the following order:
///
/// 1. **Header**: root_count, root_indexes[], node_count
/// 2. **Cell Metadata**: per-node type, refs_cnt, length
/// 3. **Edge Bitmap**: 1 bit per edge (direct successor flag)
/// 4. **Prefix Bits**: sub-byte remainders for small/pruned cells
/// 5. **Graph Deltas**: byte-boundary-aware delta encoding for non-direct edges
/// 6. **Byte Alignment**: padding to byte boundary
/// 7. **Cell Data**: actual cell payload bits
/// 8. **Final Padding**: padding to byte boundary
///
/// # Cell Type Encoding (4 bits)
///
/// | Value | Meaning                                              |
/// |-------|------------------------------------------------------|
/// | 0     | Ordinary cell                                        |
/// | 1-8   | Special cell with PrunedBranch levelmask = value - 1 |
/// | 9     | Depth-balance marker (MerkleUpdate optimization)     |
///
/// # Arguments
/// * `compressed` - Compressed data (without algorithm byte prefix)
/// * `max_size` - Maximum allowed decompressed size (security limit)
///
/// # Returns
/// Vector of root cells on success
///
/// # Errors
/// Returns error on:
/// - Input too small
/// - Decompressed size exceeds max_size
/// - Invalid graph structure (cycles, out-of-bounds references)
/// - Malformed cell data
pub fn boc_decompress_improved_structure_lz4(
    compressed: Vec<u8>,
    max_size: usize,
) -> Result<Vec<Cell>> {
    // Maximum cell data length in bits (TON limit)
    const K_MAX_CELL_DATA_LENGTH_BITS: usize = 1024;
    // Size of decompressed length header
    const K_DECOMPRESSED_SIZE_BYTES: usize = 4;

    // ========================================================================
    // SECTION 1: Outer Wrapper
    // ========================================================================
    // Wire format:
    //   +----------------+------------------+
    //   | 4 bytes BE     | Variable length  |
    //   | decompressed   | LZ4 payload      |
    //   | size           |                  |
    //   +----------------+------------------+
    //
    // C++ equivalent:
    //   size_t decompressed_size = td::BitSlice(compressed.ubegin(), kSizeBits).bits().get_uint(kSizeBits);
    //   TRY_RESULT(serialized, td::lz4_decompress(compressed, decompressed_size));
    // ========================================================================
    if compressed.len() < K_DECOMPRESSED_SIZE_BYTES {
        fail!("BOC decompression failed: input too small for header");
    }

    let size_bytes: [u8; 4] =
        compressed[0..4].try_into().map_err(|_| error!("Cannot convert size"))?;
    let decompressed_size = u32::from_be_bytes(size_bytes) as usize;
    if decompressed_size > max_size {
        fail!("BOC decompression failed: invalid decompressed size");
    }

    let serialized = lz4::block::decompress(&compressed[4..], Some(decompressed_size as i32))?;
    if serialized.len() != decompressed_size {
        fail!("BOC decompression failed: decompressed size mismatch");
    }

    // ========================================================================
    // SECTION 2: Initialize Bit Reader
    // ========================================================================
    // The inner stream is a MSB-first bitstream.
    // C++ equivalent: td::BitSlice bit_reader(serialized.as_slice().ubegin(), serialized.size() * 8);
    // ========================================================================
    let mut reader = BitSliceReader::new(&serialized);
    let orig_size_bits = serialized.len() * 8;

    // ========================================================================
    // SECTION 3: Stream Header
    // ========================================================================
    // Wire format:
    //   +-------------+---------------------------+-------------+
    //   | 32 bits     | 32 bits × root_count      | 32 bits     |
    //   | root_count  | root_indexes[]            | node_count  |
    //   +-------------+---------------------------+-------------+
    //
    // - root_count: Number of root cells in the BOC
    // - root_indexes[]: Topological rank of each root (0-indexed)
    // - node_count: Total number of unique cells in the DAG
    //
    // C++ equivalent:
    //   TRY_RESULT(root_count, read_uint(bit_reader, 32));
    //   for (int i = 0; i < root_count; ++i) { TRY_RESULT_ASSIGN(root_indexes[i], read_uint(...)); }
    //   TRY_RESULT(node_count, read_uint(bit_reader, 32));
    // ========================================================================
    let root_count = reader.read_uint(32)? as usize;
    if root_count < 1 || root_count > decompressed_size {
        fail!("BOC decompression failed: invalid root count");
    }
    let mut root_indexes = Vec::with_capacity(root_count);
    for _ in 0..root_count {
        root_indexes.push(reader.read_uint(32)? as usize);
    }
    let node_count = reader.read_uint(32)? as usize;
    if node_count < 1 {
        fail!("BOC decompression failed: invalid node count");
    }
    if node_count > decompressed_size {
        fail!("BOC decompression failed: incorrect node count provided");
    }
    for &idx in &root_indexes {
        if idx >= node_count {
            fail!("BOC decompression failed: invalid root index");
        }
    }

    // ========================================================================
    // SECTION 4: Per-Node Metadata Arrays
    // ========================================================================
    // These arrays are indexed by topological rank (0..node_count-1).
    // All cells are processed in topological order, where children have
    // higher ranks than their parents.
    // ========================================================================
    let mut cell_data_length = vec![0usize; node_count];
    let mut is_data_small = vec![false; node_count];
    let mut is_special = vec![false; node_count];
    let mut is_depth_balance = vec![false; node_count];
    let mut cell_refs_cnt = vec![0usize; node_count];
    let mut pruned_branch_level = vec![0u8; node_count];

    let mut cell_builders: Vec<BuilderData> = (0..node_count).map(|_| BuilderData::new()).collect();
    let mut boc_graph: Vec<[usize; 4]> = vec![[0usize; 4]; node_count];

    // ========================================================================
    // SECTION 5: Cell Metadata
    // ========================================================================
    // Per-node encoding (in topological order):
    //
    //   +----------+----------+-------------------+
    //   | 4 bits   | 4 bits   | 8 bits (optional) |
    //   | cell_type| refs_cnt | length field      |
    //   +----------+----------+-------------------+
    //
    // cell_type encoding:
    //   0:   Ordinary cell
    //   1-8: Special cell with PrunedBranch levelmask = (cell_type - 1)
    //   9:   Depth-balance marker (MerkleUpdate optimization, no payload)
    //
    // length field (only for non-PrunedBranch, non-depth-balance cells):
    //   +--------+--------+
    //   | 1 bit  | 7 bits |
    //   | small  | value  |
    //   +--------+--------+
    //   - small=1: value is bit count directly (0-127 bits)
    //   - small=0: value is byte count (value * 8 bits, 0 means 1024 bits)
    //
    // NOTE: The concrete special type (MerkleUpdate/MerkleProof/Library) is NOT
    // stored here. It is inferred from the first data byte during finalization.
    //
    // C++ equivalent:
    //   size_t cell_type = bit_reader.bits().get_uint(4);
    //   is_special[i] = (cell_type == 9 ? false : bool(cell_type));
    //   cell_refs_cnt[i] = bit_reader.bits().get_uint(4);
    // ========================================================================
    for i in 0..node_count {
        if reader.remaining_bits() < 8 {
            fail!("BOC decompression failed: not enough bits for cell metadata");
        }

        let cell_type = reader.read_uint(4)? as u8;
        is_depth_balance[i] = cell_type == 9;
        is_special[i] = if cell_type == 9 { false } else { cell_type != 0 };
        if is_special[i] {
            pruned_branch_level[i] = cell_type - 1;
        }

        cell_refs_cnt[i] = reader.read_uint(4)? as usize;
        if cell_refs_cnt[i] > 4 {
            fail!("BOC decompression failed: invalid cell refs count");
        }

        // Depth-balance markers have no payload
        if is_depth_balance[i] {
            cell_data_length[i] = 0;
            continue;
        }

        // PrunedBranch: length is derived from levelmask popcount
        // Formula: (256 hash bits + 16 depth bits) × popcount(levelmask)
        if pruned_branch_level[i] != 0 {
            let coef = (pruned_branch_level[i] as u32).count_ones() as usize;
            cell_data_length[i] = (256 + 16) * coef;
        } else {
            // Regular cells: read length field
            if reader.remaining_bits() < 8 {
                fail!("BOC decompression failed: not enough bits for data length");
            }
            let small = reader.read_uint(1)? == 1;
            let len_val = reader.read_uint(7)? as usize;
            is_data_small[i] = small;
            if small {
                // Small: value is exact bit count
                cell_data_length[i] = len_val;
            } else {
                // Non-small: value is byte count (including padding + marker)
                // The actual data bits will be extracted later by subtracting padding_bits
                let mut bits = len_val * 8;
                if bits == 0 {
                    bits = 1024; // 0 encodes maximum cell size
                }
                cell_data_length[i] = bits;
            }
        }

        if cell_data_length[i] > K_MAX_CELL_DATA_LENGTH_BITS {
            fail!("BOC decompression failed: invalid cell data length");
        }
    }

    // ========================================================================
    // SECTION 6: Direct Edge Bitmap
    // ========================================================================
    // One bit per edge in the graph (total: sum of refs_cnt[i] for all i).
    //
    //   1 = Direct successor: child_rank == parent_rank + 1
    //   0 = Non-direct: delta encoded in Section 8
    //
    // This optimization exploits the fact that in topological order, children
    // often immediately follow their parents, requiring only 1 bit per edge.
    //
    // C++ equivalent:
    //   for (int j = 0; j < cell_refs_cnt[i]; ++j) {
    //     TRY_RESULT(edge_connection, read_uint(bit_reader, 1));
    //     if (edge_connection) { boc_graph[i][j] = i + 1; }
    //   }
    // ========================================================================
    for i in 0..node_count {
        for child_idx in boc_graph[i].iter_mut().take(cell_refs_cnt[i]) {
            let edge = reader.read_uint(1)? == 1;
            if edge {
                *child_idx = i + 1;
            }
        }
    }

    // ========================================================================
    // SECTION 7: Prefix Bits (Sub-Byte Remainders)
    // ========================================================================
    // For cells with non-byte-aligned bit lengths, the remainder bits
    // (bit_length % 8) are stored here, before the main data section.
    //
    // This applies to:
    // - PrunedBranch cells (always have (256+16)*n bits, but header is added)
    // - Small cells with bit_length % 8 != 0
    //
    // For non-small cells, bit_length is always a multiple of 8, so
    // remainder_bits == 0 and no bits are consumed.
    //
    // For PrunedBranch cells, we also prepend the 2-byte header here:
    //   [0x01][levelmask]
    //
    // C++ equivalent:
    //   if (prunned_branch_level[i]) {
    //     cell_builders[i].store_long((1 << 8) + prunned_branch_level[i], 16);
    //   }
    //   size_t remainder_bits = cell_data_length[i] % 8;
    //   cell_builders[i].store_bits(bit_reader.subslice(0, remainder_bits));
    // ========================================================================
    for i in 0..node_count {
        if is_depth_balance[i] {
            continue;
        }
        if pruned_branch_level[i] != 0 {
            // PrunedBranch header: [0x01][levelmask]
            let header = (1u16 << 8) | (pruned_branch_level[i] as u16);
            cell_builders[i].append_u16(header)?;
            cell_builders[i].set_type(CellType::PrunedBranch);
        }
        let remainder_bits = cell_data_length[i] % 8;
        if reader.remaining_bits() < remainder_bits {
            fail!("BOC decompression failed: not enough bits for initial cell data");
        }
        reader.read_bits_to_builder(&mut cell_builders[i], remainder_bits)?;
        cell_data_length[i] -= remainder_bits;
    }

    // ========================================================================
    // SECTION 8: Graph Delta Encoding (Non-Direct Edges)
    // ========================================================================
    // For edges where child_rank != parent_rank + 1, we encode the delta:
    //   delta = child_rank - parent_rank - 2
    //
    // The encoding uses a byte-boundary-aware three-case scheme:
    //
    // Let:
    //   pref_size = consumed_bits (position in bitstream)
    //   required_bits = bit_length(max_possible_delta) = bit_length(node_count - i - 3)
    //   threshold = 8 - ((pref_size + 1) % 8) + 1 = bits until next byte + 1
    //
    // Case 1: required_bits < threshold
    //   → Encode delta directly in required_bits
    //
    // Case 2: required_bits >= threshold AND delta fits in available_bits
    //   → Encode [1][delta in available_bits]
    //   → available_bits = 8 - (pref_size + 1) % 8
    //
    // Case 3: required_bits >= threshold AND delta doesn't fit
    //   → Encode [0][delta in required_bits]
    //
    // For nodes near the end (node_count <= i + 3), missing edges default to i + 2.
    //
    // C++ equivalent: See lines 594-627 in boc-compression.cpp
    // ========================================================================
    for i in 0..node_count {
        // Small graph optimization: default missing edges to i + 2
        if node_count <= i + 3 {
            for child_idx in boc_graph[i].iter_mut().take(cell_refs_cnt[i]) {
                if *child_idx == 0 {
                    *child_idx = i + 2;
                }
            }
            continue;
        }

        let required_bits = required_bits_for_delta(node_count, i);

        for child_idx in boc_graph[i].iter_mut().take(cell_refs_cnt[i]) {
            // Skip direct edges (already set in Section 6)
            if *child_idx != 0 {
                continue;
            }

            let pref_size = reader.consumed_bits();
            let threshold = 8 - ((pref_size + 1) % 8) + 1;

            if required_bits < threshold {
                // Case 1: Direct encoding
                let delta = reader.read_uint(required_bits)? as usize;
                *child_idx = delta + i + 2;
            } else {
                // Case 2 or 3: Read flag bit first
                let edge_connection = reader.read_uint(1)? == 1;
                if edge_connection {
                    // Case 2: Compact encoding (delta fits in available bits)
                    let pref_size = reader.consumed_bits();
                    let available_bits = 8 - (pref_size % 8);
                    let delta = reader.read_uint(available_bits)? as usize;
                    *child_idx = delta + i + 2;
                } else {
                    // Case 3: Full encoding
                    let delta = reader.read_uint(required_bits)? as usize;
                    *child_idx = delta + i + 2;
                }
            }
        }
    }

    // ========================================================================
    // SECTION 9: Graph Validation
    // ========================================================================
    // Verify topological ordering invariant: children must have strictly
    // higher ranks than parents (child_rank > parent_rank).
    //
    // This prevents cycles and ensures the DAG can be built bottom-up.
    // ========================================================================
    for node in 0..node_count {
        for child in boc_graph[node].iter().take(cell_refs_cnt[node]) {
            if *child >= node_count {
                fail!("BOC decompression failed: invalid graph connection");
            }
            if *child <= node {
                fail!("BOC decompression failed: circular reference in graph");
            }
        }
    }

    // ========================================================================
    // SECTION 10: Byte Alignment
    // ========================================================================
    // Pad to byte boundary before the cell data section.
    // C++ equivalent: while ((orig_size - bit_reader.size()) % 8) { read_uint(bit_reader, 1); }
    // ========================================================================
    while !(orig_size_bits - reader.remaining_bits()).is_multiple_of(8) {
        reader.read_uint(1)?;
    }

    // ========================================================================
    // SECTION 11: Cell Data Section
    // ========================================================================
    // For each node (in topological order), read the remaining cell data bits.
    //
    // Encoding depends on cell type:
    //
    // A) Depth-balance markers (cell_type=9): Skip (no payload on wire)
    //
    // B) PrunedBranch cells: Data follows prefix bits directly
    //
    // C) Small cells (is_data_small=true): Data follows prefix bits directly
    //
    // D) Non-small ordinary/special cells: Padding + marker + data
    //    Format: 0* 1 <data_bits>
    //    - Leading zeros pad to byte alignment
    //    - '1' bit is a marker
    //    - Remaining bits are actual cell data
    //    - padding_bits = (number of zeros) + 1 (for marker)
    //
    // C++ equivalent:
    //   if (!prunned_branch_level[i] && !is_data_small[i]) {
    //     while (bit_reader.bits()[0] == 0) { ++padding_bits; bit_reader.advance(1); }
    //     read_uint(bit_reader, 1); ++padding_bits;
    //   }
    //   cell_builders[i].store_bits(bit_reader.subslice(0, remaining_data_bits));
    // ========================================================================
    for i in 0..node_count {
        // Skip depth-balance markers (no payload)
        if is_depth_balance[i] {
            continue;
        }

        let mut padding_bits = 0usize;

        // Non-small, non-PrunedBranch cells use padding + marker encoding
        if pruned_branch_level[i] == 0 && !is_data_small[i] {
            // Skip padding zeros
            while reader.remaining_bits() > 0 && reader.peek_bit() == Some(false) {
                padding_bits += 1;
                reader.skip_bits(1)?;
            }
            // Read marker bit
            reader.read_uint(1)?;
            padding_bits += 1;
        }

        if cell_data_length[i] < padding_bits {
            fail!("BOC decompression failed: invalid cell data length");
        }
        let remaining_data_bits = cell_data_length[i] - padding_bits;
        if reader.remaining_bits() < remaining_data_bits {
            fail!("BOC decompression failed: not enough bits for remaining cell data");
        }
        reader.read_bits_to_builder(&mut cell_builders[i], remaining_data_bits)?;
    }

    // ========================================================================
    // SECTION 12: Build Cell DAG
    // ========================================================================
    // Build the actual Cell objects from the parsed data.
    //
    // This section implements three key functions following C++ logic:
    //
    // 1. finalize_node(idx):
    //    - Append child references to the cell builder
    //    - Set special cell type based on first data byte
    //    - Convert builder to Cell
    //
    // 2. build_node(idx):
    //    - DFS traversal from roots
    //    - Special handling for MerkleUpdate: build left subtree first,
    //      then right subtree with paired diff computation
    //
    // 3. build_right_under_mu(right_idx, left_idx, sum_diff_out):
    //    - Reconstruct depth-balance marker (cell_type=9) nodes
    //    - Compute grams as: right_grams = left_grams + sum_child_diff
    //    - Propagate diffs upward for parent reconstruction
    //
    // C++ reference: Lines 673-770 in boc-compression.cpp
    // ========================================================================
    let mut nodes: Vec<Option<Cell>> = vec![None; node_count];
    let mut builders: Vec<Option<BuilderData>> = cell_builders.into_iter().map(Some).collect();

    // --------------------------------
    // finalize_node: Complete a cell builder and convert to Cell
    // --------------------------------
    // C++ equivalent:
    //   for (int j = 0; j < cell_refs_cnt[idx]; ++j) {
    //     cell_builders[idx].store_ref(nodes[boc_graph[idx][j]]);
    //   }
    //   nodes[idx] = cell_builders[idx].finalize(is_special[idx]);
    // --------------------------------
    let mut finalize_node = |idx: usize,
                             nodes: &Vec<Option<Cell>>,
                             builders: &mut Vec<Option<BuilderData>>|
     -> Result<Cell> {
        let mut builder = builders[idx]
            .take()
            .ok_or_else(|| error!("BOC decompression failed: builder missing"))?;
        for child_idx in boc_graph[idx].iter().take(cell_refs_cnt[idx]) {
            let child = nodes[*child_idx]
                .as_ref()
                .ok_or_else(|| error!("BOC decompression failed: child cell not yet built"))?
                .clone();
            builder.checked_append_reference(child)?;
        }
        set_special_cell_type_from_data(&mut builder, is_special[idx])?;
        builder.into_cell()
    };

    // --------------------------------
    // build_right_under_mu: Reconstruct MerkleUpdate right subtree
    // --------------------------------
    // This handles the depth-balance elision optimization (cell_type=9).
    //
    // Algorithm:
    // 1. If node already built, just compute vertex diff for parent
    // 2. Recursively build children, accumulating sum_child_diff
    // 3. If depth-balance marker: reconstruct grams = left_grams + sum_child_diff
    // 4. Finalize node and propagate diff upward
    //
    // C++ reference: Lambda `build_right_under_mu` at line 695 in boc-compression.cpp
    //
    // Note on integer types:
    // C++ uses `td::RefInt256` for arbitrary precision, but since TON Grams
    // are limited to VarUInteger 16 (≤ 120 bits), we use `i128` which is
    // sufficient for sum/diff operations.
    // --------------------------------
    #[allow(clippy::too_many_arguments)]
    fn build_right_under_mu(
        right_idx: usize,
        left_idx: Option<usize>,
        sum_diff_out: Option<&mut i128>,
        nodes: &mut Vec<Option<Cell>>,
        builders: &mut Vec<Option<BuilderData>>,
        is_depth_balance: &[bool],
        cell_refs_cnt: &[usize],
        boc_graph: &[[usize; 4]],
        finalize_node: &mut impl FnMut(
            usize,
            &Vec<Option<Cell>>,
            &mut Vec<Option<BuilderData>>,
        ) -> Result<Cell>,
    ) -> Result<()> {
        // If the right node is already built, we may still need to contribute its vertex diff
        // into the parent accumulator (this matches the C++ early-return path).
        if nodes[right_idx].is_some() {
            if let (Some(left_idx), Some(sum_out)) = (left_idx, sum_diff_out) {
                if let (Some(left_cell), Some(right_cell)) = (&nodes[left_idx], &nodes[right_idx]) {
                    if let Some(diff) = process_shard_accounts_vertex(left_cell, right_cell) {
                        *sum_out = sum_out
                            .checked_add(diff)
                            .ok_or_else(|| error!("BOC decompression failed: integer overflow"))?;
                    }
                }
            }
            return Ok(());
        }

        let mut sum_child_diff: i128 = 0;
        for j in 0..cell_refs_cnt[right_idx] {
            let right_child = boc_graph[right_idx][j];
            let left_child =
                left_idx.and_then(
                    |l| {
                        if j < cell_refs_cnt[l] {
                            Some(boc_graph[l][j])
                        } else {
                            None
                        }
                    },
                );
            build_right_under_mu(
                right_child,
                left_child,
                Some(&mut sum_child_diff),
                nodes,
                builders,
                is_depth_balance,
                cell_refs_cnt,
                boc_graph,
                finalize_node,
            )?;
        }

        let mut cur_right_left_diff: Option<i128> = None;
        if is_depth_balance[right_idx] {
            // Depth-balance marker (ct=9): the right node's payload was elided on the wire.
            // Reconstruct it as:
            //   right_grams = left_grams + sum_child_diff
            // and propagate `sum_child_diff` upward as the effective (right-left) diff.
            let left_idx = left_idx.ok_or_else(|| {
                error!("BOC decompression failed: depth-balance left vertex is missing")
            })?;
            let left_cell = nodes[left_idx]
                .as_ref()
                .ok_or_else(|| error!("BOC decompression failed: child cell not yet built"))?;
            let left_grams = extract_depth_balance_grams(left_cell).ok_or_else(|| {
                error!("BOC decompression failed: depth-balance left vertex has no grams")
            })?;
            let expected = (left_grams as i128)
                .checked_add(sum_child_diff)
                .ok_or_else(|| error!("BOC decompression failed: integer overflow"))?;
            if expected < 0 {
                fail!("BOC decompression failed: depth-balance grams became negative");
            }
            let expected_u128 = expected as u128;
            let builder = builders[right_idx]
                .as_mut()
                .ok_or_else(|| error!("BOC decompression failed: builder missing"))?;
            write_depth_balance_grams(builder, expected_u128)?;
            cur_right_left_diff = Some(sum_child_diff);
        }

        let cell = (finalize_node)(right_idx, nodes, builders)?;
        nodes[right_idx] = Some(cell);

        if cur_right_left_diff.is_none() {
            if let Some(left_idx) = left_idx {
                if let (Some(left_cell), Some(right_cell)) = (&nodes[left_idx], &nodes[right_idx]) {
                    cur_right_left_diff = process_shard_accounts_vertex(left_cell, right_cell);
                }
            }
        }

        if let (Some(sum_out), Some(diff)) = (sum_diff_out, cur_right_left_diff) {
            *sum_out = sum_out
                .checked_add(diff)
                .ok_or_else(|| error!("BOC decompression failed: integer overflow"))?;
        }

        Ok(())
    }

    // --------------------------------
    // build_node: DFS traversal to build cells
    // --------------------------------
    // Standard DFS with special handling for MerkleUpdate cells:
    // - For MerkleUpdate: build left subtree first, then right with pairing
    // - For other cells: build all children, then finalize
    //
    // C++ reference: Lambda `build_node` at line 749 in boc-compression.cpp
    // --------------------------------
    #[allow(clippy::too_many_arguments)]
    fn build_node(
        idx: usize,
        nodes: &mut Vec<Option<Cell>>,
        builders: &mut Vec<Option<BuilderData>>,
        is_special: &[bool],
        is_depth_balance: &[bool],
        cell_refs_cnt: &[usize],
        boc_graph: &[[usize; 4]],
        finalize_node: &mut impl FnMut(
            usize,
            &Vec<Option<Cell>>,
            &mut Vec<Option<BuilderData>>,
        ) -> Result<Cell>,
    ) -> Result<()> {
        // Skip if already built (handles DAG sharing)
        if nodes[idx].is_some() {
            return Ok(());
        }

        // Special case: MerkleUpdate cells
        // Build left subtree normally, then right subtree with paired diff computation
        if let Some(b) = &builders[idx] {
            if is_merkle_update_node(is_special[idx], b) {
                if cell_refs_cnt[idx] < 2 {
                    fail!("BOC decompression failed: invalid MerkleUpdate node");
                }
                let left_idx = boc_graph[idx][0];
                let right_idx = boc_graph[idx][1];

                // Build left subtree normally
                build_node(
                    left_idx,
                    nodes,
                    builders,
                    is_special,
                    is_depth_balance,
                    cell_refs_cnt,
                    boc_graph,
                    finalize_node,
                )?;

                // Build right subtree with pairing for depth-balance reconstruction
                build_right_under_mu(
                    right_idx,
                    Some(left_idx),
                    None,
                    nodes,
                    builders,
                    is_depth_balance,
                    cell_refs_cnt,
                    boc_graph,
                    finalize_node,
                )?;

                let cell = (finalize_node)(idx, nodes, builders)?;
                nodes[idx] = Some(cell);
                return Ok(());
            }
        }

        // Standard case: build all children first
        for j in 0..cell_refs_cnt[idx] {
            let child = boc_graph[idx][j];
            build_node(
                child,
                nodes,
                builders,
                is_special,
                is_depth_balance,
                cell_refs_cnt,
                boc_graph,
                finalize_node,
            )?;
        }

        let cell = (finalize_node)(idx, nodes, builders)?;
        nodes[idx] = Some(cell);
        Ok(())
    }

    // --------------------------------
    // Build from roots
    // --------------------------------
    // C++ equivalent:
    //   for (size_t index : root_indexes) { TRY_STATUS(build_node(index)); }
    // --------------------------------
    for &root_idx in &root_indexes {
        build_node(
            root_idx,
            &mut nodes,
            &mut builders,
            &is_special,
            &is_depth_balance,
            &cell_refs_cnt,
            &boc_graph,
            &mut finalize_node,
        )?;
    }

    // Collect root cells
    let mut root_cells = Vec::with_capacity(root_count);
    for idx in root_indexes {
        root_cells.push(
            nodes[idx]
                .clone()
                .ok_or_else(|| error!("BOC decompression failed: root cell not found"))?,
        );
    }
    Ok(root_cells)
}

// ============================================================================
// ImprovedStructureLZ4 Compression
// ============================================================================

/// Recursively builds the cell DAG for compression.
///
/// This function traverses the cell tree depth-first, creating a deduplicated
/// graph representation keyed by `repr_hash`. For each unique cell, it stores:
/// - Adjacency list (child references)
/// - Raw data bits
/// - Cell type and PrunedBranch levelmask
///
/// # MerkleUpdate Depth-Balance Optimization
/// When traversing MerkleUpdate cells (used in ShardAccounts), the encoder
/// compares left and right subtrees. For right-subtree nodes where:
///   `sum_of_child_diffs == vertex_diff`
/// the cell data can be elided (pruned_branch_level = 9) and reconstructed
/// during decompression from left_grams + sum_child_diff.
///
/// # C++ Reference
/// ```cpp
/// const auto build_graph = [&](auto&& self, td::Ref<vm::Cell> cell, ...) -> td::Result<size_t>
/// ```
/// File: `docs/ton-node-cpp/crypto/vm/boc-compression.cpp`, lines 145-221
///
/// # Arguments
/// * `cell` - The cell to process
/// * `left_cell` - Optional paired left cell for MerkleUpdate optimization
/// * `under_mu_right` - Whether we're in the right subtree of a MerkleUpdate
/// * `sum_diff_out` - Output accumulator for child diffs (MerkleUpdate optimization)
/// * `graph` - Output adjacency list (child indices for each node)
/// * `refs_cnt` - Output reference count for each node
/// * `cell_data` - Output raw data bits for each node
/// * `cell_type` - Output cell type for each node
/// * `pruned_branch_level` - Output PrunedBranch levelmask (0 for non-pruned, 9 for depth-balance)
/// * `visited` - Hash map for deduplication (repr_hash → node index)
///
/// # Returns
/// The node index assigned to this cell
#[allow(clippy::too_many_arguments)]
fn build_graph_recursive(
    cell: &Cell,
    left_cell: Option<&Cell>,
    under_mu_right: bool,
    sum_diff_out: Option<&mut i128>,
    graph: &mut Vec<[usize; 4]>,
    refs_cnt: &mut Vec<usize>,
    cell_data: &mut Vec<CellBits>,
    cell_type: &mut Vec<CellType>,
    pruned_branch_level: &mut Vec<u8>,
    visited: &mut HashMap<UInt256, usize>,
) -> Result<usize> {
    // Check if already visited (DAG deduplication)
    let cell_hash = cell.repr_hash();
    if let Some(&id) = visited.get(&cell_hash) {
        return Ok(id);
    }

    // Assign new node ID
    let current_cell_id = graph.len();
    visited.insert(cell_hash, current_cell_id);

    let data = cell.data();
    let bit_len = cell.bit_length();
    let size_refs = cell.references_count();
    let special_type = cell.cell_type();

    if size_refs > 4 {
        fail!("Too many references");
    }

    graph.push([0; 4]);
    refs_cnt.push(size_refs);
    cell_type.push(special_type);

    // Extract cell data
    if special_type == CellType::PrunedBranch {
        // PrunedBranch: skip the 2-byte header [0x01][levelmask]
        // The header is reconstructed during decompression
        if bit_len < 16 || data.len() < 2 {
            fail!("Invalid PrunedBranch cell");
        }
        let payload_bits = bit_len - 16;
        let payload_bytes = payload_bits.div_ceil(8);
        if data.len() < 2 + payload_bytes {
            fail!("Invalid PrunedBranch cell data");
        }
        cell_data.push(CellBits::new(data[2..2 + payload_bytes].to_vec(), payload_bits));
        pruned_branch_level.push(data[1]); // levelmask
    } else {
        // Regular cells: copy all data bits
        let bytes_len = bit_len.div_ceil(8);
        if data.len() < bytes_len {
            fail!("Invalid cell data");
        }
        let mut bytes = data[..bytes_len].to_vec();
        // Clear unused bits in last byte
        let rem = bit_len % 8;
        if rem != 0 {
            let mask = 0xFFu8 << (8 - rem);
            if let Some(last) = bytes.last_mut() {
                *last &= mask;
            }
        }
        cell_data.push(CellBits::new(bytes, bit_len));
        pruned_branch_level.push(0u8);
    }

    // Process cell references with MerkleUpdate optimization
    let is_special = special_type != CellType::Ordinary;

    if special_type == CellType::MerkleUpdate && size_refs == 2 {
        // MerkleUpdate: traverse left normally, right with paired left for optimization
        let left_child = cell.reference(0)?;
        let right_child = cell.reference(1)?;

        // Left branch: traverse normally
        let left_child_id = build_graph_recursive(
            &left_child,
            None,
            false,
            None,
            graph,
            refs_cnt,
            cell_data,
            cell_type,
            pruned_branch_level,
            visited,
        )?;
        graph[current_cell_id][0] = left_child_id;

        // Right branch: traverse paired with left for depth-balance optimization
        let right_child_id = build_graph_recursive(
            &right_child,
            Some(&left_child),
            true,
            None,
            graph,
            refs_cnt,
            cell_data,
            cell_type,
            pruned_branch_level,
            visited,
        )?;
        graph[current_cell_id][1] = right_child_id;
    } else if under_mu_right && left_cell.is_some() {
        // We're in the right subtree of a MerkleUpdate, paired with left_cell
        let left = left_cell.unwrap();
        let mut sum_child_diff: i128 = 0;

        // Recursively process children paired with left's children
        let left_refs = left.references_count();
        for i in 0..size_refs {
            let child = cell.reference(i)?;
            let left_child = if i < left_refs { Some(left.reference(i)?) } else { None };

            let child_id = build_graph_recursive(
                &child,
                left_child.as_ref(),
                true,
                Some(&mut sum_child_diff),
                graph,
                refs_cnt,
                cell_data,
                cell_type,
                pruned_branch_level,
                visited,
            )?;
            graph[current_cell_id][i] = child_id;
        }

        // Compute this vertex's diff and check if we can elide
        let vertex_diff = process_shard_accounts_vertex(left, cell);
        if !is_special {
            if let Some(vd) = vertex_diff {
                if vd == sum_child_diff {
                    // Can elide this cell's data - it can be reconstructed
                    cell_data[current_cell_id] = CellBits::new(vec![], 0);
                    pruned_branch_level[current_cell_id] = 9;
                }
            }
        }

        // Propagate diff to parent
        if let (Some(out), Some(vd)) = (sum_diff_out, vertex_diff) {
            *out += vd;
        }
    } else {
        // Normal traversal
        for i in 0..size_refs {
            let child = cell.reference(i)?;
            let child_id = build_graph_recursive(
                &child,
                None,
                false,
                None,
                graph,
                refs_cnt,
                cell_data,
                cell_type,
                pruned_branch_level,
                visited,
            )?;
            graph[current_cell_id][i] = child_id;
        }
    }

    Ok(current_cell_id)
}

/// Compresses BOC roots using the C++-compatible ImprovedStructureLZ4 algorithm.
///
/// This encoder produces output that is byte-for-byte compatible with the C++
/// implementation for standard cell structures.
///
/// # C++ Reference
/// ```cpp
/// td::Result<td::BufferSlice>
/// boc_compress_improved_structure_lz4(const std::vector<td::Ref<vm::Cell>>& boc_roots)
/// ```
/// File: `docs/ton-node-cpp/crypto/vm/boc-compression.cpp`, lines 122-422
///
/// # Encoder Notes
/// - This encoder is wire-compatible with C++ including the MerkleUpdate optimization
/// - The encoder supports depth-balance elision (`cell_type = 9`) for ShardAccounts
///   nodes where the grams value can be reconstructed from children's diffs.
///
/// # Algorithm Overview
/// 1. Build deduplicated cell DAG from roots
/// 2. Topological sort with C++-compatible tie-breaking
/// 3. Serialize in sections: header, metadata, edges, data
/// 4. LZ4 compress the serialized bitstream
///
/// # Wire Format
/// See module-level documentation for the complete format specification.
pub fn boc_compress_improved_structure_lz4(boc_roots: Vec<Cell>) -> Result<Vec<u8>> {
    if boc_roots.is_empty() {
        fail!("No root cells were provided for serialization");
    }

    // ========================================================================
    // PHASE 1: Build Cell DAG
    // ========================================================================
    let mut boc_graph: Vec<[usize; 4]> = Vec::new();
    let mut refs_cnt: Vec<usize> = Vec::new();
    let mut cell_data: Vec<CellBits> = Vec::new();
    let mut cell_type: Vec<CellType> = Vec::new();
    let mut pruned_branch_level: Vec<u8> = Vec::new();
    let mut root_indexes: Vec<usize> = Vec::new();
    let mut visited: HashMap<UInt256, usize> = HashMap::new();

    for root in &boc_roots {
        let root_cell_id = build_graph_recursive(
            root,
            None,  // left_cell
            false, // under_mu_right
            None,  // sum_diff_out
            &mut boc_graph,
            &mut refs_cnt,
            &mut cell_data,
            &mut cell_type,
            &mut pruned_branch_level,
            &mut visited,
        )?;
        root_indexes.push(root_cell_id);
    }

    let node_count = boc_graph.len();

    // ========================================================================
    // PHASE 2: Build Reverse Graph and Compute Metadata
    // ========================================================================
    let mut reverse_graph: Vec<Vec<usize>> = vec![Vec::new(); node_count];
    let mut edge_count = 0usize;
    for i in 0..node_count {
        for child in boc_graph[i].iter_mut().take(refs_cnt[i]) {
            edge_count += 1;
            reverse_graph[*child].push(i);
        }
    }

    // Determine "small" flag: cells with < 128 bits use direct bit length encoding
    let mut is_data_small = vec![false; node_count];
    for i in 0..node_count {
        if cell_type[i] != CellType::PrunedBranch {
            is_data_small[i] = cell_data[i].bit_len < 128;
        }
    }

    // ========================================================================
    // PHASE 3: Topological Sort (C++ Compatible)
    // ========================================================================
    // The sort must match C++ for wire compatibility.
    //
    // Tie-breaking rules (in priority order):
    // 1. Ordinary cells before special cells
    // 2. Larger cells before smaller cells (by bit length)
    // 3. Lower node ID before higher node ID
    //
    // C++ uses a tuple comparison: (cell_type == 0, -cell_data.size(), -node_id)
    // ========================================================================
    let mut topo_order: Vec<usize> = Vec::with_capacity(node_count);
    let mut rank: Vec<usize> = vec![0; node_count];
    let mut queue: Vec<(i32, i32, i32)> = Vec::with_capacity(node_count);
    let mut in_degree: Vec<usize> = vec![0; node_count];

    // Initialize: nodes with no children (leaves) start in queue
    for i in 0..node_count {
        in_degree[i] = refs_cnt[i];
        if in_degree[i] == 0 {
            let is_ord = (cell_type[i] == CellType::Ordinary) as i32;
            let size_neg = -(cell_data[i].bit_len as i32);
            let id_neg = -(i as i32);
            queue.push((is_ord, size_neg, id_neg));
        }
    }
    if queue.is_empty() {
        fail!("Cycle detected in cell references");
    }
    queue.sort();

    // Process in reverse topological order (children before parents)
    while let Some((_a, _b, id_neg)) = queue.pop() {
        let node = (-id_neg) as usize;
        topo_order.push(node);
        for &parent in &reverse_graph[node] {
            in_degree[parent] -= 1;
            if in_degree[parent] == 0 {
                queue.push((0, 0, -(parent as i32)));
            }
        }
    }
    if topo_order.len() != node_count {
        fail!("Invalid graph structure");
    }
    topo_order.reverse(); // Now parents come before children

    // Compute rank for each node
    for (i, &node) in topo_order.iter().enumerate() {
        rank[node] = i;
    }

    // ========================================================================
    // PHASE 4: Serialize Bitstream
    // ========================================================================
    let mut out = BitStringWriter::with_capacity_bits(node_count * 16 + edge_count + 1024);

    // --------------------------------
    // Section: Header
    // --------------------------------
    out.push_uint(root_indexes.len() as u64, 32);
    for &root in &root_indexes {
        out.push_uint(rank[root] as u64, 32);
    }
    out.push_uint(node_count as u64, 32);

    // --------------------------------
    // Section: Cell Metadata
    // --------------------------------
    for node in topo_order.iter().take(node_count) {
        let node = *node;
        let is_sp = cell_type[node] != CellType::Ordinary;
        let current_cell_type = (is_sp as u8) + pruned_branch_level[node];
        out.push_uint(current_cell_type as u64, 4);
        out.push_uint(refs_cnt[node] as u64, 4);

        // Length field (skip for PrunedBranch and depth-balance markers)
        if cell_type[node] != CellType::PrunedBranch && current_cell_type != 9 {
            if is_data_small[node] {
                // Small: 1 bit flag + 7 bits exact length
                out.push_uint(1, 1);
                out.push_uint(cell_data[node].bit_len as u64, 7);
            } else {
                // Non-small: 0 bit flag + 7 bits byte count
                // Encodes as (1 + floor(bits/8)), handles padding + marker
                let bytes = cell_data[node].bit_len / 8;
                out.push_uint(0, 1);
                out.push_uint((1 + bytes) as u64, 7);
            }
        }
    }

    // --------------------------------
    // Section: Edge Bitmap
    // --------------------------------
    for (i, node) in topo_order.iter().take(node_count).enumerate() {
        let node = *node;
        for child in boc_graph[node].iter().take(refs_cnt[node]) {
            // 1 = direct successor (child_rank == parent_rank + 1)
            out.push_bit(rank[*child] == i + 1);
        }
    }

    // --------------------------------
    // Section: Prefix Bits
    // --------------------------------
    // Sub-byte remainders for small/pruned cells
    for &node in &topo_order {
        if pruned_branch_level[node] == 9 {
            continue; // Depth-balance marker
        }
        if cell_type[node] != CellType::PrunedBranch && !is_data_small[node] {
            continue; // Non-small cells have byte-aligned lengths
        }
        let rem = cell_data[node].bit_len % 8;
        out.push_bits(&cell_data[node].bytes, 0, rem);
    }

    // --------------------------------
    // Section: Graph Deltas
    // --------------------------------
    // Byte-boundary-aware delta encoding for non-direct edges
    for (i, node) in topo_order.iter().take(node_count).enumerate() {
        let node = *node;
        if node_count <= i + 3 {
            continue; // Small graph optimization
        }
        let required_bits = required_bits_for_delta(node_count, i);
        for child in boc_graph[node].iter().take(refs_cnt[node]) {
            let child = *child;
            if rank[child] <= i + 1 {
                continue; // Direct or self-reference (should not happen)
            }
            let delta = rank[child] - i - 2;
            let cur = out.len_bits();
            let threshold = 8 - ((cur + 1) % 8) + 1;

            if required_bits < threshold {
                // Case 1: Direct encoding
                out.push_uint(delta as u64, required_bits);
            } else {
                let avail = 8 - ((cur + 1) % 8);
                if delta < (1usize << avail) {
                    // Case 2: Compact encoding [1][delta]
                    out.push_uint(1, 1);
                    out.push_uint(delta as u64, avail);
                } else {
                    // Case 3: Full encoding [0][delta]
                    out.push_uint(0, 1);
                    out.push_uint(delta as u64, required_bits);
                }
            }
        }
    }

    // --------------------------------
    // Section: Byte Alignment
    // --------------------------------
    while !out.len_bits().is_multiple_of(8) {
        out.push_uint(0, 1);
    }

    // --------------------------------
    // Section: Cell Data
    // --------------------------------
    for &node in &topo_order {
        if pruned_branch_level[node] == 9 {
            continue; // Depth-balance marker (no data)
        }
        if cell_type[node] == CellType::PrunedBranch || is_data_small[node] {
            // Small/Pruned: remaining bits after prefix
            let prefix = cell_data[node].bit_len % 8;
            out.push_bits(&cell_data[node].bytes, prefix, cell_data[node].bit_len - prefix);
        } else {
            // Non-small: padding + marker + data
            let data_size = cell_data[node].bit_len + 1; // +1 for marker
            let padding = (8 - (data_size % 8)) % 8;
            if padding != 0 {
                out.push_zeros(padding);
            }
            out.push_uint(1, 1); // Marker bit
            out.push_bits(&cell_data[node].bytes, 0, cell_data[node].bit_len);
        }
    }

    // --------------------------------
    // Section: Final Padding
    // --------------------------------
    while !out.len_bits().is_multiple_of(8) {
        out.push_uint(0, 1);
    }

    // ========================================================================
    // PHASE 5: LZ4 Compress
    // ========================================================================
    let serialized = out.data;
    let mut compressed = lz4::block::compress(&serialized, None, true)?;
    // Replace LZ4's size header with big-endian decompressed size
    let size: Vec<u8> = (serialized.len() as u32).to_be_bytes().to_vec();
    compressed[0..4].copy_from_slice(&size);
    Ok(compressed)
}

#[cfg(test)]
#[path = "tests/test_boc_compression.rs"]
mod tests;
