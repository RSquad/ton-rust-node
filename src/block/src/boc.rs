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
    cell::{self},
    crc32_digest, error, fail, level_mask, BocCellDraft, ByteOrderRead, Cell, CellLoader,
    CellsArena, Crc32, Result, SliceData, UInt256, DEPTH_SIZE, MAX_DATA_BYTES,
    MAX_REFERENCES_COUNT, MAX_SAFE_DEPTH, SHA256_SIZE,
};
use std::{
    collections::{hash_map, HashMap, HashSet},
    fmt::Debug,
    fs::File,
    io::{Cursor, Read, Seek, SeekFrom, Write},
    ops::BitOrAssign,
    path::{Path, PathBuf},
    sync::Arc,
};

const BOC_INDEXED_TAG: u32 = 0x68ff65f3; // deprecated, is used only for read
const BOC_INDEXED_CRC32_TAG: u32 = 0xacc3a728; // deprecated, is used only for read
const BOC_GENERIC_TAG: u32 = 0xb5ee9c72;

const MAX_ROOTS_COUNT: usize = 1024;
const MAX_CELL_WEIGHT: u32 = 0xFF;
const MAX_CELL_DISTR_WEIGHT: u32 = 64;

// TODO: rename
pub trait CellsStorage {
    fn load_cell(&self, hash: &UInt256) -> Result<Cell>;
    fn load_cell_data(
        &self,
        hash: &UInt256,
        write_hashes: bool,
        dest: &mut dyn Write,
    ) -> Result<()>;
}

struct IntermediateState {
    file: File,
    cell_sizes: Vec<u16>,
    ref_count: usize,
    total_size: u64,
    raw_cell_size: usize,
}

const FILE_BUFFER_LEN: usize = 128 * 1024 * 1024; // 128 MB
const TEMP_REF_SIZE: usize = std::mem::size_of::<u32>();
const CHECK_ABORT_EACH: usize = 0x3FF; // 1023

struct RemoveOnDrop(PathBuf);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.0) {
            log::warn!("failed to remove temp file: {} error: {e:?}", self.0.display());
        }
    }
}
pub struct BocWriterStack {}

impl BocWriterStack {
    //only single root boc
    //max cells count is u32::MAX
    pub fn write<T: Write, S: CellsStorage>(
        dest: &mut T,
        temp_dir: &Path,
        root_cell: Cell,
        max_depth: u16,
        cells_storage: S,
        abort: &dyn Fn() -> bool,
    ) -> Result<()> {
        //check root_cell
        if root_cell.virtualization() != 0 {
            fail!("Virtual cells serialisation is prohibited");
        }
        let depth = root_cell.repr_depth();
        if depth > max_depth {
            fail!("Cell {:x} is too deep: {} > {}", root_cell.repr_hash(), depth, max_depth);
        }
        let file_name = format!("temp_boc_{:x}", root_cell.repr_hash());
        let tmp_file_path = temp_dir.join(file_name);
        let tmp_file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&tmp_file_path)?;
        let remove_on_drop = RemoveOnDrop(tmp_file_path);

        let mut state = Self::traverse(tmp_file, root_cell, cells_storage, abort)?;

        //write to the main file
        let cells_count = state.cell_sizes.len();
        let ref_size = BocWriter::number_of_bytes_to_fit(state.cell_sizes.len());

        let total_cells_size = state.raw_cell_size + state.ref_count * ref_size;
        let offset_size = BocWriter::number_of_bytes_to_fit(total_cells_size);

        debug_assert!(ref_size <= 4);
        debug_assert!(offset_size <= 8);

        // Header
        let magic = BOC_GENERIC_TAG;
        dest.write_all(&magic.to_be_bytes())?;

        // has index | has CRC | has cache bits | flags   | ref_size
        // 7         | 6       | 5              | 4 3     | 2 1 0
        dest.write_all(&[ref_size as u8])?;

        dest.write_all(&[offset_size as u8])?; // off_bytes:(## 8) { off_bytes <= 8 }
        dest.write_all(&(cells_count as u64).to_be_bytes()[(8 - ref_size)..8])?;
        dest.write_all(&(1_u64).to_be_bytes()[(8 - ref_size)..8])?;
        dest.write_all(&0_u64.to_be_bytes()[(8 - ref_size)..8])?;
        dest.write_all(&(total_cells_size as u64).to_be_bytes()[(8 - offset_size)..8])?;

        // Root's indexes
        dest.write_all(&(0u64).to_be_bytes()[(8 - ref_size)..8])?;

        // Cells
        let mut cell_buffer =
            [0; 2 + 4 * (SHA256_SIZE + DEPTH_SIZE) + MAX_DATA_BYTES + 4 * TEMP_REF_SIZE];
        for &cell_size in state.cell_sizes.iter().rev() {
            check_abort(abort)?;
            state.total_size -= cell_size as u64;
            state.file.seek(SeekFrom::Start(state.total_size))?;
            state.file.read_exact(&mut cell_buffer[..cell_size as usize])?;

            //let slice = &mut cell_buffer[0..TEMP_REF_SIZE];
            //let ref_count = u32::from_be_bytes(slice.try_into().unwrap());
            let ref_count = cell::refs_count(&cell_buffer);

            let data_size = cell_size as usize - ref_count * TEMP_REF_SIZE;
            let ref_offset = data_size;

            let raw_data_slice = &cell_buffer[..data_size];
            dest.write_all(raw_data_slice)?;

            for r in 0..ref_count {
                let ref_offset = ref_offset + r * TEMP_REF_SIZE;
                let slice = &mut cell_buffer[ref_offset..ref_offset + TEMP_REF_SIZE];
                let index = u32::from_be_bytes(slice.try_into().unwrap());
                let child_index = cells_count as u64 - index as u64 - 1;
                dest.write_all(&(child_index as u64).to_be_bytes()[(8 - ref_size)..8])?;
            }
        }

        drop(state);
        drop(remove_on_drop);

        Ok(())
    }

    //move throught boc and prepare data for writing to the main file
    //write cells to the temp file and collect aditional info
    fn traverse<S: CellsStorage>(
        file: File,
        root_cell: Cell,
        cells_storage: S,
        abort: &dyn Fn() -> bool,
    ) -> Result<IntermediateState> {
        enum StackItem {
            New(Cell),
            Loaded(LoadedCell),
        }
        struct LoadedCell {
            cell: Cell,
            //Indeses of cild cells in the file
            references: smallvec::SmallVec<[u32; 4]>,
        }

        let mut temp_file_buffer = std::io::BufWriter::with_capacity(FILE_BUFFER_LEN, file);

        //map of cell hashes to indeces and written flag
        let mut indices: HashMap<UInt256, (u32, bool)> = HashMap::default();
        //map of indeces to iteration.
        //when we write some cell to file we put it to remap
        //Key: the cell with index(u32) from indeces
        //Value: the cell serial number(u32 as [u8; 4]) into the file
        let mut remap: HashMap<u32, [u8; 4]> = HashMap::default();
        //stack of cells to process
        //we use stack because we need to write child cells first
        let mut stack: Vec<(u32, StackItem)> = Vec::with_capacity(32);

        //vec of cell sizes
        //todo we can write cell size after cell. So at the end of file we will have the size of last cell
        let mut cell_sizes: Vec<u16> = Vec::<u16>::with_capacity(FILE_BUFFER_LEN);
        //total size of all cells and its references
        let mut total_size: u64 = 0;
        //all references count in boc
        let mut ref_count = 0;
        //iteration is index of cell in the file
        let mut iteration = 0u32;
        //index in the remao map
        let mut remap_index = 0u32;
        //total size of all cells without references
        let mut raw_cell_size = 0usize;

        let mut max_stack_len: usize = 0;

        indices.insert(root_cell.repr_hash().clone(), (iteration, false));
        stack.push((iteration, StackItem::New(root_cell)));

        while let Some((index, item)) = stack.pop() {
            check_abort(abort)?;
            if stack.len() > max_stack_len {
                max_stack_len = stack.len();
            }
            match item {
                StackItem::New(cell) => {
                    let mut reference_indices = smallvec::SmallVec::with_capacity(4);

                    //references that are not written in the file yet
                    let mut cells: Vec<(u32, Cell)> = Vec::with_capacity(4);

                    for i in 0..cell.references_count() {
                        let index = match indices.entry(cell.reference_repr_hash(i)?) {
                            hash_map::Entry::Vacant(entry) => {
                                remap_index += 1;
                                entry.insert((remap_index, false));
                                cells.push((
                                    remap_index,
                                    cells_storage.load_cell(&cell.reference_repr_hash(i)?)?,
                                ));
                                remap_index
                            }
                            hash_map::Entry::Occupied(entry) => {
                                let (remap_index, written) = *entry.get();
                                if !written {
                                    cells.push((
                                        remap_index,
                                        cells_storage.load_cell(&cell.reference_repr_hash(i)?)?,
                                    ));
                                }
                                remap_index
                            }
                        };

                        reference_indices.push(index);
                    }

                    stack.push((
                        index,
                        StackItem::Loaded(LoadedCell { cell, references: reference_indices }),
                    ));

                    for (index, cell) in cells {
                        stack.push((index, StackItem::New(cell)));
                    }
                }
                StackItem::Loaded(loaded) => {
                    match remap.entry(index) {
                        hash_map::Entry::Vacant(entry) => {
                            entry.insert(iteration.to_be_bytes());
                        }
                        hash_map::Entry::Occupied(_) => continue,
                    };

                    if let Some((_, written)) = indices.get_mut(loaded.cell.repr_hash()) {
                        *written = true;
                    }

                    iteration += 1;

                    //update counters
                    let raw_cell = cell::full_len(loaded.cell.raw_data()?);
                    let cell_size = raw_cell + TEMP_REF_SIZE * loaded.cell.references_count();
                    cell_sizes.push(cell_size as u16);
                    ref_count += loaded.cell.references_count();
                    total_size += cell_size as u64;
                    raw_cell_size += raw_cell;

                    //write cella and references to the temp file
                    temp_file_buffer.write_all(loaded.cell.raw_data()?)?;
                    for index in loaded.references.iter() {
                        let index = remap.get(index).ok_or_else(|| error!("index not found"))?;
                        temp_file_buffer.write_all(index)?;
                    }
                }
            }
        }

        let mut file = temp_file_buffer.into_inner()?;
        file.flush()?;
        Ok(IntermediateState { file, cell_sizes, ref_count, total_size, raw_cell_size })
    }
}

#[derive(PartialEq, Eq)]
enum RevisitingMode {
    Previsit,
    Visit,
    Arrange,
}

// | hashes | refs  | flags |
// | count  | count |       |
// | 7 6    | 5 4 3 | 2 1 0 |
#[derive(Clone, Copy)]
pub struct CellInfoFlags(u8);
impl CellInfoFlags {
    const IS_ROOT: Self = Self(1);
    const SHOULD_CACHE: Self = Self(2);
    const STORES_HASHES: Self = Self(4);
    fn new(cell: &Cell) -> Self {
        let mut flags = if cell.store_hashes() { Self::STORES_HASHES } else { Self(0) };
        flags.0 |= (cell.references_count() as u8) << 3;
        flags.0 |= cell.level() << 6;
        flags
    }
    #[inline(always)]
    fn contains(&self, flag: CellInfoFlags) -> bool {
        (self.0 & flag.0) != 0
    }
}
impl BitOrAssign for CellInfoFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

const REVISITING_NEW: i32 = -1;
const REVISITING_PREVISITED: i32 = -2;
const REVISITING_VISITED: i32 = -3;

#[derive(Clone)]
struct CellInfo<T: Clone> {
    cell: T,
    flags: CellInfoFlags,
    weight: u8,
    data_size: u8,
    refs: [u32; MAX_REFERENCES_COUNT], // Children position in `cells` list
    revisiting_status: i32,
    new_cell_index: i32,
}

impl<T: Clone> CellInfo<T> {
    fn with_cell(cell: Cell, weight: u8, refs: [u32; MAX_REFERENCES_COUNT]) -> CellInfo<Cell> {
        CellInfo {
            data_size: cell.data().len() as u8,
            flags: CellInfoFlags::new(&cell),
            cell,
            weight,
            refs,
            revisiting_status: -1,
            new_cell_index: -1,
        }
    }

    fn with_hash(cell: &Cell) -> CellInfo<UInt256> {
        CellInfo {
            data_size: cell.data().len() as u8,
            flags: CellInfoFlags::new(cell),
            cell: cell.repr_hash().clone(),
            weight: 0,
            refs: [0; MAX_REFERENCES_COUNT],
            revisiting_status: -1,
            new_cell_index: -1,
        }
    }

    #[inline(always)]
    fn is_arranged(rs: i32) -> bool {
        rs >= 0
    }
    #[inline(always)]
    fn hashes_count(&self) -> usize {
        ((self.flags.0 & 0b1100_0000) >> 6) as usize + 1
    }
    #[inline(always)]
    fn refs_count(&self) -> usize {
        ((self.flags.0 & 0b0011_1000) >> 3) as usize
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct BocFlags: u8 {
        const None = 0;
        const Index = 1;
        const Crc32 = 2;
        const TopHash = 4;
        const IntHashes = 8;
        const CacheBits = 16;
    }
}

#[derive(Clone)]
pub struct ChunkedVec<T: Clone, const S: usize> {
    pub chunks: smallvec::SmallVec<[Vec<T>; 1]>,
}

impl<T: Clone, const S: usize> Default for ChunkedVec<T, S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone, const S: usize> ChunkedVec<T, S> {
    pub fn new() -> Self {
        Self { chunks: smallvec::SmallVec::new() }
    }

    pub fn push(&mut self, value: T) {
        if self.chunks.is_empty() || self.chunks.last().unwrap().len() >= S {
            self.chunks.push(Vec::new());
        }
        self.chunks.last_mut().unwrap().push(value);
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        self.chunks.get(index / S).and_then(|c| c.get(index % S))
    }

    pub fn len(&self) -> usize {
        if self.chunks.is_empty() {
            0
        } else {
            (self.chunks.len() - 1) * S + self.chunks.last().map_or(0, |chunk| chunk.len())
        }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &T> {
        self.chunks.iter().flat_map(|chunk| chunk.iter())
    }
}

impl<T: Clone, const S: usize> core::ops::Index<usize> for ChunkedVec<T, S> {
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        &self.chunks[index / S][index % S]
    }
}

impl<T: Clone, const S: usize> core::ops::IndexMut<usize> for ChunkedVec<T, S> {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.chunks[index / S][index % S]
    }
}

pub fn write_boc(root_cell: &Cell) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    BocWriter::with_root(root_cell)?.write(&mut buf)?;
    Ok(buf)
}

pub fn write_boc_multi(roots: Vec<Cell>) -> Result<Vec<u8>> {
    if roots.is_empty() {
        return Ok(Vec::new());
    }
    let mut buf = Vec::new();
    BocWriter::with_roots(roots)?.write(&mut buf)?;
    Ok(buf)
}

#[macro_export]
macro_rules! define_BocWriter {
    ( $writer_type_name:ident, $cell_info_type:ty, $cell_list_type:ty, $( $field_name:ident : $field_ty:ty ),* ) => {

        #[derive(Clone)]
        pub struct $writer_type_name<'a> {
            flags: BocFlags,
            roots_indexes_rev: Vec<usize>,
            // reversed list of cells
            // (the cells will be written to boc in reverse order)
            cells: $cell_list_type,
            // map of cells hashes to their position in `cells` list
            cells_index: ahash::AHashMap<UInt256, u32>,
            data_size: usize,
            references: usize,
            stored_hashes: usize,
            abort: &'a dyn Fn() -> bool,
            $(
                $field_name : $field_ty,
            )*
        }

        impl<'a> $writer_type_name<'a> {

            pub fn with_params(
                root_cells: impl IntoIterator<Item = Cell>,
                max_depth: u16,
                mut flags: BocFlags,
                abort: &'a dyn Fn() -> bool,
                $(
                    $field_name : $field_ty,
                )*
            ) -> Result<Self> {

                //
                // In cpp node implementation, flag 'cell_info.is_root' is always false due to a bug.
                // (see https://github.com/ton-blockchain/ton/blob/master/crypto/vm/boc.cpp#L305)
                // So BocFlags::TopHash flag is not work whenewer it set or not.
                //
                flags.remove(BocFlags::TopHash);
                //

                let mut boc = Self {
                    flags,
                    roots_indexes_rev: Vec::new(),
                    cells: <$cell_list_type>::new(),
                    cells_index: ahash::AHashMap::new(),
                    data_size: 0,
                    references: 0,
                    stored_hashes: 0,
                    abort,
                    $(
                        $field_name,
                    )*
                };
                let mut roots_set = HashSet::new();
                for root_cell in root_cells {
                    if root_cell.virtualization() != 0 {
                        fail!("Virtual cells serialisation is prohibited");
                    }
                    let root_hash = root_cell.repr_hash().clone();
                    if !roots_set.insert(root_hash.clone()) {
                        fail!("roots must be all unique")
                    }
                    let depth = root_cell.repr_depth();
                    if depth > max_depth {
                        fail!("Cell {:x} is too deep: {} > {}", root_hash, depth, max_depth);
                    }

                    if let Some(rev_index) = boc.cells_index.get(&root_hash) {
                        boc.roots_indexes_rev.push(*rev_index as usize);
                    } else {
                        let rev_index = boc.arrange_cells(root_cell)?;
                        boc.roots_indexes_rev.push(rev_index as usize);
                    }
                }

                boc.distribute_weights()?;

                boc.rearrange_cells()?;

                Ok(boc)
            }

            pub fn roots_count(&self) -> usize {
                self.roots_indexes_rev.len()
            }

            pub fn data_size(&self) -> usize {
                self.data_size
            }

            pub fn references_count(&self) -> usize {
                self.references
            }

            pub fn cells_count(&self) -> usize {
                self.cells.len()
            }

            pub fn write<T: Write>(self, dest: &mut T) -> Result<()> {
                if self.flags.contains(BocFlags::Crc32) {
                    let mut dest_wrapped = IoCrcFilter::new_writer(dest);
                    self.write_ex_impl(&mut dest_wrapped)?;
                    dest_wrapped.finalize()
                } else {
                    self.write_ex_impl(dest)
                }
            }

            pub fn write_to_vec(self) -> Result<Vec<u8>> {
                let size = self.calc_boc_size();
                let mut v = Vec::with_capacity(size);
                self.write(&mut v)?;
                Ok(v)
            }

            pub fn write_to_file(self, path: impl AsRef<Path>) -> Result<()> {
                let mut file = File::create(path)?;
                self.write(&mut file)
            }

            pub fn calc_boc_size(&self) -> usize {
                let (ref_size, total_cells_size, offset_size) =
                    self.compute_serialization_params();
                let header_size = 4 + 1 + 1 // magic + flags_byte + offset_size_byte
                    + 3 * ref_size           // cells_count + roots_count + absent_count
                    + offset_size;           // total_cells_size field
                let roots_size = ref_size * self.roots_count();
                let index_size = if self.flags.contains(BocFlags::Index) {
                    self.cells_count() * offset_size
                } else {
                    0
                };
                let crc_size = if self.flags.contains(BocFlags::Crc32) { 4 } else { 0 };
                header_size + roots_size + index_size + total_cells_size + crc_size
            }

            fn compute_serialization_params(&self) -> (usize, usize, usize) {
                let ref_size = Self::number_of_bytes_to_fit(self.cells_count());
                let mut total_cells_size = self.data_size + self.references * ref_size;
                if self.flags.contains(BocFlags::IntHashes) || self.flags.contains(BocFlags::TopHash) {
                    total_cells_size += self.stored_hashes * (SHA256_SIZE + DEPTH_SIZE);
                }
                let include_cache_bits = self.flags.contains(BocFlags::CacheBits);
                let offset_size = Self::number_of_bytes_to_fit(
                    if include_cache_bits { total_cells_size * 2 } else { total_cells_size });
                (ref_size, total_cells_size, offset_size)
            }

            fn write_ex_impl<T: Write>(self, dest: &mut T) -> Result<()> {

                let (ref_size, total_cells_size, offset_size) =
                    self.compute_serialization_params();
                let include_cache_bits = self.flags.contains(BocFlags::CacheBits);

                debug_assert!(ref_size <= 4);
                debug_assert!(offset_size <= 8);

                // Header

                let magic = BOC_GENERIC_TAG;
                dest.write_all(&magic.to_be_bytes())?;

                // has index | has CRC | has cache bits | flags   | ref_size
                // 7         | 6       | 5              | 4 3     | 2 1 0
                let include_index = self.flags.contains(BocFlags::Index);
                let include_crc = self.flags.contains(BocFlags::Crc32);
                dest.write_all(&[
                        (include_index as u8) << 7 |
                        (include_crc as u8) << 6 |
                    (include_cache_bits as u8) << 5 |
                            ref_size as u8
                ])?;

                dest.write_all(&[offset_size as u8])?; // off_bytes:(## 8) { off_bytes <= 8 }
                dest.write_all(&(self.cells_count() as u64).to_be_bytes()[(8-ref_size)..8])?;
                dest.write_all(&(self.roots_count() as u64).to_be_bytes()[(8-ref_size)..8])?;
                dest.write_all(&0_u64.to_be_bytes()[(8-ref_size)..8])?;
                dest.write_all(&(total_cells_size as u64).to_be_bytes()[(8-offset_size)..8])?;

                // Root's indexes
                for index in self.roots_indexes_rev.iter() {
                    check_abort(self.abort)?;
                    dest.write_all(&((self.cells_count() - *index - 1) as u64).to_be_bytes()[(8-ref_size)..8])?;
                }

                // Index
                if include_index {
                    let mut offset = 0;
                    for (i, cell_info) in self.cells.iter().rev().enumerate() {
                        let cell_info = &self.cells[cell_info.new_cell_index as usize];
                        if i & CHECK_ABORT_EACH == 0 {
                            check_abort(self.abort)?;
                        }
                        offset += self.calc_cell_len(cell_info, ref_size);
                        let mut value = offset;
                        if self.flags.contains(BocFlags::CacheBits) {
                            value <<= 1;
                            if cell_info.flags.contains(CellInfoFlags::SHOULD_CACHE) {
                                value += 1;
                            }
                        }
                        dest.write_all(&(value as u64).to_be_bytes()[(8-offset_size)..8])?;
                    }
                }

                // Cells
                for (cell_index, cell_info) in self.cells.iter().rev().enumerate() {
                    if cell_index & CHECK_ABORT_EACH == 0 {
                        check_abort(self.abort)?;
                    }
                    let cell_info = &self.cells[cell_info.new_cell_index as usize];

                    self.write_cell_data(cell_info, dest)?;
                    for i in 0..cell_info.refs_count() {
                        let child_index = self.cells_count() - 1 - cell_info.refs[i as usize] as usize;
                        //write!(&mut log, " {} ", child_index).unwrap();
                        debug_assert!(child_index > cell_index);
                        dest.write_all(&(child_index as u64).to_be_bytes()[(8-ref_size)..8])?;
                    }
                }

                Ok(())
            }

            // This function distributes cell weights to form virtual subtrees within the overall tree.
            // Each subtree has approximately the same number of cells,
            // while the tree topology can be very different.
            // The division is based on pre-initialized weights (see arrange_cells).
            // Virtual subtrees are formed by cell weights - the weight is zero in the leaves,
            // and maximum in the root.
            fn distribute_weights(&mut self) -> Result<()> {

                for i in (0..self.cells.len()).rev() {
                    let cell_info = &self.cells[i];
                    let refs = cell_info.refs_count() as usize;
                    let mut fat_children =
                        smallvec::SmallVec::<[usize; MAX_REFERENCES_COUNT]>::new();
                    let mut sum = MAX_CELL_DISTR_WEIGHT - 1;

                    // Collect child cells with weight > limit
                    for j in 0..refs {
                        let child_info = &self.cells[cell_info.refs[j] as usize];
                        let limit = (MAX_CELL_DISTR_WEIGHT - 1 + j as u32) / refs as u32;
                        if child_info.weight as u32 <= limit {
                            sum -= child_info.weight as u32;
                        } else {
                            fat_children.push(cell_info.refs[j] as usize);
                        }
                    }

                    // Distribute limited weight between fat children
                    for j in &fat_children {
                        let limit = sum / fat_children.len() as u32;
                        sum += 1;
                        let child_info = &mut self.cells[*j];
                        child_info.weight = child_info.weight.min(limit as u8);
                    }

                    if i & CHECK_ABORT_EACH == 0 {
                        check_abort(self.abort)?;
                    }
                }

                let flags = self.flags;
                for i in 0..self.cells.len() {
                    let cell_info = &self.cells[i];
                    let mut children_weight = 1;
                    for j in 0..cell_info.refs_count() {
                        children_weight += self.cells[cell_info.refs[j as usize] as usize].weight;
                    }

                    debug_assert!(children_weight <= MAX_CELL_DISTR_WEIGHT as u8);

                    let cell_info = &mut self.cells[i];
                    if children_weight <= cell_info.weight {
                        cell_info.weight = children_weight;
                    } else {
                        // Cell with zero weight will be stored with hashes
                        cell_info.weight = 0;
                        if flags.contains(BocFlags::IntHashes) {
                            self.stored_hashes += cell_info.hashes_count();
                        }
                    }

                    if i & CHECK_ABORT_EACH == 0 {
                        check_abort(self.abort)?;
                    }
                }

                for i in 0..self.roots_indexes_rev.len() {
                    let cell_info = &mut self.cells[self.roots_indexes_rev[i]];
                    cell_info.flags |= CellInfoFlags::IS_ROOT;
                    if flags.contains(BocFlags::TopHash) &&
                    (!flags.contains(BocFlags::IntHashes) || cell_info.weight != 0)
                    {
                        self.stored_hashes += cell_info.hashes_count();
                    }

                    check_abort(self.abort)?;
                }

                Ok(())
            }

            // Tree traversal where cells are arranged based on previously formed virtual subtrees
            // (see distribute_weights). As a result, all cells of one subtree will be located together.
            fn rearrange_cells(&mut self) -> Result<()> {

                // Most of the work is done in the first 'revisit' call.
                // The second and third calls are needed to complete the formation of subtrees
                // at the root of the whole tree,
                // these calls do not go deeper than the first special cells (cells with zero weight).

                let mut new_cells_index = 0;
                for i in 0..self.roots_indexes_rev.len() {
                    let root_index = self.roots_indexes_rev[i];
                    self.revisit(root_index, RevisitingMode::Previsit, &mut new_cells_index)?;
                    self.revisit(root_index, RevisitingMode::Visit, &mut new_cells_index)?;
                }

                let mut new_roots_indexes = Vec::with_capacity(self.roots_indexes_rev.len());
                for i in 0..self.roots_indexes_rev.len() {
                    let root_index = self.roots_indexes_rev[i];

                    let new_root_index = self.revisit(root_index, RevisitingMode::Arrange, &mut new_cells_index)?;
                    if <$cell_info_type>::is_arranged(new_root_index) {
                        new_roots_indexes.push(new_root_index as usize);
                    } else {
                        fail!("INTERNAL ERROR: root cell #{} is not arranged", root_index);
                    }
                }

                debug_assert!(new_cells_index == self.cells.len() as i32);

                self.roots_indexes_rev = new_roots_indexes;
                Ok(())
            }

            fn revisit(
                &mut self,
                cell_index: usize,
                mode: RevisitingMode,
                new_cells_index: &mut i32,
            ) -> Result<i32> {

                debug_assert!(cell_index < self.cells.len());

                if cell_index & CHECK_ABORT_EACH == 0 {
                    check_abort(self.abort)?;
                }

                let rs = self.cells[cell_index].revisiting_status;

                if <$cell_info_type>::is_arranged(rs) {
                    return Ok(rs)
                }

                // 1) Previsit. Going down (from root to leaves)
                if mode == RevisitingMode::Previsit {
                    if rs != REVISITING_NEW {
                        return Ok(rs)
                    }

                    for i in (0..self.cells[cell_index].refs_count()).rev() {
                        let child_index = self.cells[cell_index].refs[i as usize] as usize;
                        let child_mode = if self.cells[child_index].weight == 0 {
                            // 2) Special processing for virtual trees leaves (cells with hashes)
                            RevisitingMode::Visit
                        } else {
                            RevisitingMode::Previsit
                        };
                        self.revisit(child_index, child_mode, new_cells_index)?;
                    }

                    self.cells[cell_index].revisiting_status = REVISITING_PREVISITED;
                    return Ok(REVISITING_PREVISITED);
                }

                // 7) Arrange
                if mode == RevisitingMode::Arrange {
                    self.cells[cell_index].revisiting_status = *new_cells_index;
                    self.cells[*new_cells_index as usize].new_cell_index = cell_index as i32;
                    *new_cells_index += 1;
                    return Ok(*new_cells_index - 1);
                }

                if self.cells[cell_index].revisiting_status == REVISITING_VISITED {
                    return Ok(REVISITING_VISITED);
                }

                // 3) Continue going down
                if self.cells[cell_index].weight == 0 {
                    self.revisit(cell_index, RevisitingMode::Previsit, new_cells_index)?;
                }

                // 4) Visit. Going down until
                //    - leaves
                //    - already visited cells
                //    - already arranged cells
                //    - cells with hashes
                for i in (0..self.cells[cell_index].refs_count()).rev() {
                    let child_index = self.cells[cell_index].refs[i as usize] as usize;
                    self.revisit(child_index, RevisitingMode::Visit, new_cells_index)?;
                }

                // 6) Arrange. From leaves to roots
                for i in (0..self.cells[cell_index].refs_count()).rev() {
                    let child_index = self.cells[cell_index].refs[i as usize] as usize;
                    let child_rs = self.revisit(child_index, RevisitingMode::Arrange, new_cells_index)?;

                    if <$cell_info_type>::is_arranged(child_rs) {
                        self.cells[cell_index].refs[i as usize] = child_rs as u32;
                    } else {
                        fail!("INTERNAL ERROR: child cell #{} is not arranged", i);
                    }
                }

                // 5) Leaf cell is achived
                self.cells[cell_index].revisiting_status = REVISITING_VISITED;

                Ok(REVISITING_VISITED)
            }

            #[inline(always)]
            fn update_counters(&mut self, cell: &Cell) -> Result<()> {
                // Do not count stored hashes here, it counted separately
                self.data_size += cell.data().len() + 2;
                self.references += cell.references_count();
                Ok(())
            }

            fn number_of_bytes_to_fit(l: usize) -> usize {
                let mut n = 0;
                let mut l1 = l;

                while l1 != 0 {
                    l1 >>= 8;
                    n += 1;
                }

                n
            }

            #[inline(always)]
            fn calc_cell_len(&self, cell: &$cell_info_type, ref_size: usize) -> usize {
                let mut len = 2 + cell.data_size as usize + ref_size * cell.refs_count() as usize;
                let needs_hashes =
                    self.flags.contains(BocFlags::IntHashes) && cell.weight == 0 ||
                    self.flags.contains(BocFlags::TopHash) && cell.flags.contains(CellInfoFlags::IS_ROOT);
                if needs_hashes {
                    len += (SHA256_SIZE + DEPTH_SIZE) * cell.hashes_count()
                }
                len
            }
        }
    }
}

macro_rules! define_BocWriter_cell_impl {
    ($writer:ident) => {
        impl $writer<'_> {
            pub fn with_root(root_cell: &Cell) -> Result<Self> {
                Self::with_roots([root_cell.clone()])
            }

            pub fn with_roots(root_cells: impl IntoIterator<Item = Cell>) -> Result<Self> {
                fn default_abort() -> bool {
                    false
                }
                Self::with_params(root_cells, MAX_SAFE_DEPTH, BocFlags::None, &default_abort)
            }

            pub fn with_flags(
                root_cells: impl IntoIterator<Item = Cell>,
                flags: BocFlags,
            ) -> Result<Self> {
                fn default_abort() -> bool {
                    false
                }
                Self::with_params(root_cells, MAX_SAFE_DEPTH, flags, &default_abort)
            }

            // Primary tree traversal where cells are arranged into a list (cells_list).
            // Duplicate cells are added to the list only once.
            // Child cells are always placed before their parents.
            // Cell weights are initialized by the total number of child cells (recursively) + 1,
            // but no more than 255.
            fn arrange_cells(&mut self, cell: Cell) -> Result<u32> {
                let repr_hash = cell.repr_hash().clone();

                if cell.virtualization() != 0 {
                    fail!("Virtual cells serialization is prohibited");
                }

                // TODO try to use cells_index.entry api
                if let Some(i) = self.cells_index.get(&repr_hash).cloned() {
                    self.cells[i as usize].flags |= CellInfoFlags::SHOULD_CACHE;
                    return Ok(i);
                }

                let mut weight = 1;
                let mut refs = [0; MAX_REFERENCES_COUNT];
                for (i, child_cell) in cell.clone_references()?.into_iter().enumerate() {
                    let child_rev_index = self.arrange_cells(child_cell)?;
                    refs[i] = child_rev_index;
                    weight += self.cells[child_rev_index as usize].weight as u32;
                }

                self.update_counters(&cell)?;

                let cell_info =
                    CellInfo::<Cell>::with_cell(cell, weight.min(MAX_CELL_WEIGHT) as u8, refs);

                let rev_index = self.cells.len();
                self.cells.push(cell_info);
                self.cells_index.insert(repr_hash, rev_index as u32);

                if rev_index & CHECK_ABORT_EACH == 0 {
                    check_abort(self.abort)?;
                }

                Ok(rev_index as u32)
            }

            fn write_cell_data<T: Write>(&self, cell: &CellInfo<Cell>, dest: &mut T) -> Result<()> {
                // Cell layout:
                // [D1] [D2] (hashes: 0..4 big endian u256) (depths: 0..4 big endian u16) [data: 0..128 bytes]

                let cell_raw_data = cell.cell.raw_data()?;
                let needs_hashes = self.flags.contains(BocFlags::IntHashes) && cell.weight == 0
                    || self.flags.contains(BocFlags::TopHash)
                        && cell.flags.contains(CellInfoFlags::IS_ROOT);
                let has_hashes = cell::store_hashes(cell_raw_data);
                match (needs_hashes, has_hashes) {
                    (true, true) | (false, false) => {
                        // write as is
                        dest.write_all(cell_raw_data)?;
                    }
                    (true, false) => {
                        // repack with hashes
                        let d1 = cell::calc_d1(
                            cell::level_mask(cell_raw_data),
                            true,
                            cell::cell_type(cell_raw_data),
                            cell::refs_count(cell_raw_data),
                        );
                        dest.write_all(&[d1])?;
                        dest.write_all(&cell_raw_data[1..2])?; // D2
                        for hash in cell.cell.hashes() {
                            dest.write_all(hash.as_slice())?;
                        }
                        for depth in cell.cell.depths() {
                            dest.write_all(&depth.to_be_bytes())?;
                        }
                        dest.write_all(&cell_raw_data[2..])?; // data
                    }
                    (false, true) => {
                        // repack without hashes
                        let d1 = cell::calc_d1(
                            cell::level_mask(cell_raw_data),
                            false,
                            cell::cell_type(cell_raw_data),
                            cell::refs_count(cell_raw_data),
                        );
                        dest.write_all(&[d1])?;
                        dest.write_all(&cell_raw_data[1..2])?; // D2
                        let hashes_len =
                            (SHA256_SIZE + DEPTH_SIZE) * cell::hashes_count(cell_raw_data);
                        dest.write_all(&cell_raw_data[2 + hashes_len..])?; // data
                    }
                }
                Ok(())
            }
        }
    };
}

define_BocWriter! {BocWriter, CellInfo<Cell>, Vec<CellInfo<Cell>>, }
define_BocWriter_cell_impl!(BocWriter);

// Chunk size for ChunkedBocWriter. CellInfo<Cell> is ~40 bytes, so one chunk is ~320 KB:
// large enough to keep per-chunk overhead and index arithmetic low, small enough to avoid
// the costly reallocations a single growing Vec would incur on the trees PSS produces.
const CHUNKED_BOC_WRITER_CHUNK_SIZE: usize = 8192;

// Chunk size for BigBocWriter, which streams trees from key-value storage and can hold
// millions of CellInfo<UInt256> (~64 bytes each). Smaller 1024-element chunks (~64 KB)
// trade a bit of indexing overhead for lower peak allocation pressure on huge trees.
const BIG_BOC_WRITER_CHUNK_SIZE: usize = 1024;

define_BocWriter! {ChunkedBocWriter, CellInfo<Cell>, ChunkedVec<CellInfo<Cell>, CHUNKED_BOC_WRITER_CHUNK_SIZE>, }
define_BocWriter_cell_impl!(ChunkedBocWriter);

// This implementation is optimised for working with big tree of cells stored in key-value.
define_BocWriter! {BigBocWriter, CellInfo<UInt256>, ChunkedVec<CellInfo<UInt256>, BIG_BOC_WRITER_CHUNK_SIZE>, cells_storage: Arc<dyn CellsStorage> }
impl BigBocWriter<'_> {
    // Primary tree traversal where cells are arranged into a list (cells_list).
    // Duplicate cells are added to the list only once.
    // Child cells are always placed before their parents.
    // Cell weights are initialized by the total number of child cells (recursively) + 1,
    // but no more than 255.
    fn arrange_cells(&mut self, cell: Cell) -> Result<u32> {
        let repr_hash = cell.repr_hash();
        let _ = cell;
        self.arrange_cells_(repr_hash)
    }

    fn arrange_cells_(&mut self, repr_hash: &UInt256) -> Result<u32> {
        //
        // TODO: use hash_raw_entry api.
        // It will be possible not to calculate the hash twice when hash_raw_entry becomes stable.
        //
        // let mut hasher = self.cells_index.hasher().build_hasher();
        // hasher.write(repr_hash.as_slice());
        // let short_hash = hasher.finish();

        // if let Some((_, i)) = self.cells_index.raw_entry().from_key_hashed_nocheck(short_hash, &repr_hash) {
        //     self.cells[i as usize].flags |= CellInfoFlags::SHOULD_CACHE;
        //     return Ok(i);
        // }

        check_abort(self.abort)?;

        if let Some(i) = self.cells_index.get(repr_hash).cloned() {
            self.cells[i as usize].flags |= CellInfoFlags::SHOULD_CACHE;
            return Ok(i);
        }

        let cell = self.cells_storage.load_cell(repr_hash)?;

        if cell.virtualization() != 0 {
            fail!("Virtual cells serialization is prohibited");
        }

        let mut cell_info = CellInfo::<UInt256>::with_hash(&cell);

        let mut refs = smallvec::SmallVec::<[UInt256; MAX_REFERENCES_COUNT]>::new();
        for i in 0..cell.references_count() {
            refs.push(cell.reference_repr_hash(i)?);
        }
        self.update_counters(&cell)?;

        let _ = cell;

        cell_info.weight = 1;
        for (i, child_hash) in refs.iter().enumerate() {
            let child_rev_index = self.arrange_cells_(child_hash)?;
            cell_info.refs[i] = child_rev_index;
            cell_info.weight =
                cell_info.weight.saturating_add(self.cells[child_rev_index as usize].weight);
        }

        let rev_index = self.cells.len() as u32;
        self.cells.push(cell_info);
        // self.cells_index.raw_entry_mut().from_key_hashed_nocheck(short_hash, &repr_hash) . . .
        //
        self.cells_index.insert(repr_hash.clone(), rev_index);

        Ok(rev_index)
    }

    fn write_cell_data<T: Write>(&self, cell: &CellInfo<UInt256>, dest: &mut T) -> Result<()> {
        let needs_hashes = self.flags.contains(BocFlags::IntHashes) && cell.weight == 0
            || self.flags.contains(BocFlags::TopHash)
                && cell.flags.contains(CellInfoFlags::IS_ROOT);
        self.cells_storage.load_cell_data(&cell.cell, needs_hashes, dest)
    }
}

#[inline(always)]
fn check_abort(abort: &dyn Fn() -> bool) -> Result<()> {
    if abort() {
        fail!("Operation was aborted");
    }
    Ok(())
}

/// Read a big-endian unsigned integer of `size` bytes from `data[offset..]`.
#[inline(always)]
fn read_be_uint_at(data: &[u8], offset: usize, size: usize) -> u64 {
    debug_assert!(size > 0 && size <= 8);
    debug_assert!(offset + size <= data.len());
    let slice = &data[offset..offset + size];
    let mut v = 0u64;
    for &b in slice {
        v = (v << 8) | b as u64;
    }
    v
}

#[derive(Clone)]
pub struct RawCell {
    pub data: Vec<u8>,
    pub refs: [u32; 4],
}

#[derive(Debug, Clone, Default)]
pub struct BocHeader {
    pub magic: u32,
    pub roots_count: usize,
    pub ref_size: usize,
    pub index_included: bool,
    pub cells_count: usize,
    pub offset_size: usize,
    pub has_crc: bool,
    pub has_cache_bits: bool,
    pub roots_indexes: Vec<u32>,
    pub tot_cells_size: usize,
}

pub struct BocReaderResult {
    pub roots: Vec<Cell>,
    pub header: BocHeader,
    pub flags: BocFlags,
    pub interrupted: bool,
}

impl BocReaderResult {
    pub fn withdraw_single_root(mut self) -> Result<Cell> {
        match self.roots.len() {
            0 => fail!("Error parsing cells tree: empty root"),
            1 => Ok(self.roots.remove(0)),
            r => fail!("Error parsing cells tree: too many roots {}", r),
        }
    }
}

pub trait CellsTempStorage {
    fn load_hash_and_depth(&self, index: u32) -> Result<(UInt256, u16)>;
    fn load_cell(&self, index: u32) -> Result<Cell>;
    fn store_cell(&mut self, index: u32, cell: &Cell) -> Result<()>;
    fn cleanup(&mut self) -> Result<()>;
    fn loader(&self) -> &CellLoader;
}

enum BocIndex<'a> {
    Own(Vec<u64>),
    Ref(&'a [u8], u64),
}

impl BocIndex<'_> {
    fn offset(&self, index: usize, header: &BocHeader) -> Result<usize> {
        match self {
            BocIndex::Ref(data, start) => {
                let mut offset = *start;
                if index != 0 {
                    let o = (index - 1) * header.offset_size;
                    let mut o2 = Cursor::new(&data[o..o + header.offset_size])
                        .read_be_uint(header.offset_size)?;
                    if header.has_cache_bits {
                        o2 >>= 1;
                    }
                    offset =
                        offset.checked_add(o2).ok_or_else(|| error!("cell offset overflow"))?;
                }
                Ok(offset as usize)
            }
            BocIndex::Own(i) => Ok(i[index] as usize),
        }
    }
}

pub struct BocReader<'a> {
    abort: &'a dyn Fn() -> bool,
    load_cell_callback: &'a dyn Fn(&Cell) -> bool,
    max_depth: u16,
    arena: Option<Arc<CellsArena>>,
}

impl Default for BocReader<'_> {
    fn default() -> Self {
        Self {
            abort: &|| false,
            load_cell_callback: &|_| true,
            max_depth: MAX_SAFE_DEPTH,
            arena: None,
        }
    }
}

pub fn read_boc(data: impl AsRef<[u8]>) -> Result<BocReaderResult> {
    BocReader::new().read(data.as_ref())
}

pub fn read_boc_file(path: &str) -> Result<BocReaderResult> {
    let mut file = File::open(path)?;
    BocReader::new().stream_read(&mut file)
}

pub fn read_single_root_boc(data: impl AsRef<[u8]>) -> Result<Cell> {
    read_boc(data)?.withdraw_single_root()
}

pub fn read_single_root_boc_file(path: &str) -> Result<Cell> {
    let mut file = File::open(path)?;
    BocReader::new().stream_read(&mut file)?.withdraw_single_root()
}

/// reads only single root cell from boc
pub fn read_boc_root(data: impl AsRef<[u8]>) -> Result<SliceData> {
    let (_header, slice) = BocReader::new().read_root(data.as_ref())?;
    if slice.remaining_references() != 0 {
        fail!("Boc root cell got references");
    }
    Ok(slice)
}

impl<'a> BocReader<'a> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_abort(mut self, abort: &'a dyn Fn() -> bool) -> Self {
        self.abort = abort;
        self
    }

    pub fn set_load_cell_callback(mut self, load_cell_callback: &'a dyn Fn(&Cell) -> bool) -> Self {
        self.load_cell_callback = load_cell_callback;
        self
    }

    pub fn set_arena(mut self, arena: Arc<CellsArena>) -> Self {
        self.arena = Some(arena);
        self
    }

    pub fn set_max_cell_depth(mut self, max_depth: u16) -> Self {
        self.max_depth = max_depth;
        self
    }

    pub fn stream_read<T: Read + Seek>(&mut self, src: &mut T) -> Result<BocReaderResult> {
        let position = src.stream_position()?;
        let src_full_len = src.seek(SeekFrom::End(0))? - position;
        src.seek(SeekFrom::Start(position))?;

        let (header, flags) = self.read_header(src)?;
        let header_len = src.stream_position()? - position;

        check_abort(self.abort)?;

        Self::precheck_cells_tree_len(&header, header_len, src_full_len, true)?;

        let result = if header.has_crc {
            // Seek back and re-read header through CRC filter
            // so the header bytes are included in the hash
            src.seek(SeekFrom::Start(position))?;
            let mut crc_reader = IoCrcFilter::new_reader(src);
            const SKIP_BUF_LEN: usize = 256;
            let mut skip_buf = [0u8; SKIP_BUF_LEN];
            let mut remaining = header_len as usize;
            while remaining > 0 {
                let n = remaining.min(SKIP_BUF_LEN);
                crc_reader.read_exact(&mut skip_buf[..n])?;
                remaining -= n;
            }
            let result = self.stream_read_impl(&mut crc_reader, header, flags)?;
            if !result.interrupted {
                crc_reader.check_crc()?;
            }
            result
        } else {
            self.stream_read_impl(src, header, flags)?
        };

        // Reposition the underlying stream to the exact end of this BOC,
        // so the caller can read the next BOC from the same stream.
        let header = &result.header;
        let index_size =
            header.index_included as u64 * (header.cells_count as u64 * header.offset_size as u64);
        let boc_len =
            header_len + index_size + header.tot_cells_size as u64 + header.has_crc as u64 * 4;
        src.seek(SeekFrom::Start(position + boc_len))?;

        Ok(result)
    }

    fn stream_read_impl<T: Read + Seek>(
        &mut self,
        src: &mut T,
        header: BocHeader,
        mut flags: BocFlags,
    ) -> Result<BocReaderResult> {
        // Skip index
        if header.index_included {
            // It is need to *read* (not just seek) because of crc filter

            const PART_LEN: usize = 4 * 1024;
            let mut part = [0; PART_LEN];
            let mut rest = header
                .cells_count
                .checked_mul(header.offset_size)
                .ok_or_else(|| error!("index size overflow"))?;
            while rest > 0 {
                let cur_len = rest.min(PART_LEN);
                src.read_exact(&mut part[..cur_len])?;
                rest -= cur_len;
            }
        }

        let mut draft_cells: Vec<BocCellDraft> = Vec::with_capacity(header.cells_count);
        let mut done_cells: Vec<Cell> = Vec::with_capacity(header.cells_count);

        let read_ref: fn(&mut T, &mut [u8; 4]) -> std::io::Result<u32> = match header.ref_size {
            1 => |src, buf| {
                src.read_exact(&mut buf[..1])?;
                Ok(buf[0] as u32)
            },
            2 => |src, buf| {
                src.read_exact(&mut buf[..2])?;
                Ok(u16::from_be_bytes([buf[0], buf[1]]) as u32)
            },
            3 => |src, buf| {
                src.read_exact(&mut buf[..3])?;
                Ok(u32::from_be_bytes([0, buf[0], buf[1], buf[2]]))
            },
            4 => |src, buf| {
                src.read_exact(buf)?;
                Ok(u32::from_be_bytes(*buf))
            },
            _ => fail!("invalid ref_size {}", header.ref_size),
        };

        // Read cells into drafts
        let mut actual_data_size = src.stream_position()?;
        for cell_index in 0..header.cells_count {
            if cell_index & CHECK_ABORT_EACH == 0 {
                check_abort(self.abort)?;
            }
            let draft_cell =
                Cell::read_boc_draft(src, cell_index, header.cells_count, &self.arena, &read_ref)?;
            draft_cells.push(draft_cell);
        }
        actual_data_size = src.stream_position()? - actual_data_size;
        if actual_data_size as usize != header.tot_cells_size {
            fail!("actual data size disagrees with the size from header")
        }
        let mut interrupted = false;
        for cell_index in (0..header.cells_count).rev() {
            if cell_index & CHECK_ABORT_EACH == 0 {
                check_abort(self.abort)?;
            }
            let draft =
                draft_cells.pop().ok_or_else(|| error!("Cell #{} was not found", cell_index))?;

            let rc = draft.refs_count();
            if draft.has_stored_hashes() && !header.roots_indexes.contains(&(cell_index as u32)) {
                flags |= BocFlags::IntHashes;
            }

            let mut refs = smallvec::SmallVec::<[Cell; 4]>::new();
            for i in 0..rc {
                let ref_idx = header.cells_count - 1 - draft.ref_index(i) as usize;
                refs.push(
                    done_cells
                        .get(ref_idx)
                        .ok_or_else(|| error!("Can't get ref {ref_idx}"))?
                        .clone(),
                );
            }

            let cell =
                Cell::from_boc_draft(draft, refs, Some(self.max_depth), self.arena.as_ref())?;
            if !self.push_cell(cell, &mut done_cells) {
                interrupted = true;
                break;
            }
        }

        let roots = Self::collect_roots(&header, &done_cells, &mut flags, interrupted)?;

        Ok(BocReaderResult { roots, header, flags, interrupted })
    }

    /// Optimized in-memory BOC deserialization.
    pub fn read(&mut self, data: &[u8]) -> Result<BocReaderResult> {
        // Header
        let mut cursor = Cursor::new(data);
        let (header, mut flags) = self.read_header(&mut cursor)?;
        let header_len = cursor.position() as usize;

        check_abort(self.abort)?;

        Self::precheck_cells_tree_len(&header, header_len as u64, data.len() as u64, false)?;

        // CRC32-C: single call on the whole buffer
        if header.has_crc {
            let crc = crc32_digest(&data[..data.len() - 4]);
            let stored = u32::from_le_bytes(
                data[data.len() - 4..].try_into().map_err(|_| error!("data too short for CRC"))?,
            );
            if crc != stored {
                fail!("crc not the same, values: {}, {}", stored, crc);
            }
        }

        // Build cell offsets
        let cells_data_start = if header.index_included {
            header_len
                .checked_add(
                    header
                        .cells_count
                        .checked_mul(header.offset_size)
                        .ok_or_else(|| error!("index size overflow"))?,
                )
                .ok_or_else(|| error!("cells_data_start overflow"))?
        } else {
            header_len
        };

        let offsets = if header.index_included {
            let mut v = Vec::with_capacity(header.cells_count);
            let idx_base = header_len;
            let mut prev: usize = 0;
            for i in 0..header.cells_count {
                if i & CHECK_ABORT_EACH == 0 {
                    check_abort(self.abort)?;
                }

                let off = cells_data_start
                    .checked_add(prev)
                    .ok_or_else(|| error!("cell offset overflow at cell {}", i))?;
                if off > data.len() {
                    fail!("cell offset out of bounds at cell {}", i);
                }
                v.push(off);

                let mut entry =
                    read_be_uint_at(data, idx_base + i * header.offset_size, header.offset_size);
                if header.has_cache_bits {
                    entry >>= 1;
                }
                let entry = entry as usize;
                if entry > header.tot_cells_size {
                    fail!("cell index entry out of bounds at cell {}", i);
                }
                if entry < prev {
                    fail!("cell index entries are not monotonic at cell {}", i);
                }
                if entry - prev < 2 {
                    fail!("cell too small in index at cell {}", i);
                }
                prev = entry;
            }
            if prev != header.tot_cells_size {
                fail!("actual data size disagrees with the size from header");
            }
            v
        } else {
            let mut v = Vec::with_capacity(header.cells_count);
            let mut pos = cells_data_start;
            for i in 0..header.cells_count {
                if i & CHECK_ABORT_EACH == 0 {
                    check_abort(self.abort)?;
                }
                if pos + 2 > data.len() {
                    fail!("data truncated at cell {}", i);
                }
                v.push(pos);
                let d = &data[pos..];
                pos += cell::full_len(d) + cell::refs_count(d) * header.ref_size;
            }
            if pos - cells_data_start != header.tot_cells_size {
                fail!("actual data size disagrees with the size from header");
            }
            v
        };

        // Single-pass cell construction (reverse: leaves → roots)
        let cells_data_end = cells_data_start + header.tot_cells_size;
        let mut cells: Vec<Cell> = Vec::with_capacity(header.cells_count);
        let mut interrupted = false;

        for i in (0..header.cells_count).rev() {
            if i & CHECK_ABORT_EACH == 0 {
                check_abort(self.abort)?;
            }

            let off = offsets[i];
            let raw = &data[off..];

            let wire_len = cell::full_len(raw);
            let rc = cell::refs_count(raw);
            if off + wire_len + rc * header.ref_size > cells_data_end {
                fail!("cell {} wire data exceeds cells area", i);
            }

            // BOC-specific stricter tag-completion check
            if raw[1] & 1 != 0 && wire_len > 2 && raw[wire_len - 1] & 0x7f == 0 {
                fail!("overly long tag-completed encoding at cell {}", i);
            }

            // Track IntHashes flag
            if cell::store_hashes(raw) && !header.roots_indexes.contains(&(i as u32)) {
                flags |= BocFlags::IntHashes;
            }

            // Resolve child references directly from already-built cells
            let refs_off = off + wire_len;
            let mut refs = smallvec::SmallVec::<[Cell; MAX_REFERENCES_COUNT]>::new();
            for k in 0..rc {
                let ref_idx =
                    read_be_uint_at(data, refs_off + k * header.ref_size, header.ref_size) as usize;
                if ref_idx >= header.cells_count || ref_idx <= i {
                    fail!(
                        "reference out of range: cell {}, ref {}, cells_count {}",
                        i,
                        ref_idx,
                        header.cells_count
                    );
                }
                let rev = header.cells_count - 1 - ref_idx;
                refs.push(cells[rev].clone());
            }

            // Build cell (validates data, allocates, computes hashes, writes refs)
            let cell = Cell::with_data_and_refs(
                raw,
                true,
                &refs,
                Some(self.max_depth),
                self.arena.clone(),
            )?;
            if !self.push_cell(cell, &mut cells) {
                interrupted = true;
                break;
            }
        }

        // Collect roots
        let roots = Self::collect_roots(&header, &cells, &mut flags, interrupted)?;

        Ok(BocReaderResult { roots, header, flags, interrupted })
    }

    pub fn read_to_storage(
        &mut self,
        data: &[u8],
        cells_storage: &mut dyn CellsTempStorage,
    ) -> Result<BocReaderResult> {
        #[cfg(not(target_family = "wasm"))]
        let now = std::time::Instant::now();
        let mut src = Cursor::new(data);

        let (header, mut flags) = self.read_header(&mut src)?;

        Self::precheck_cells_tree_len(&header, src.position(), data.len() as u64, false)?;

        // Index processing - read existing index or traverse all vector to create own index2
        #[cfg(not(target_family = "wasm"))]
        let now1 = std::time::Instant::now();
        let index = self.read_index(data, &mut src, &header)?;
        #[cfg(not(target_family = "wasm"))]
        let index_time = now1.elapsed().as_millis();

        // Resolving references & constructing cells from leaves to roots
        #[cfg(not(target_family = "wasm"))]
        let now1 = std::time::Instant::now();
        let mut interrupted = false;
        for cell_index in (0..header.cells_count).rev() {
            check_abort(self.abort)?;

            let offset = index.offset(cell_index, &header)?;

            if offset + 2 > data.len() {
                fail!("Invalid data: data too short or index is invalid");
            }

            let mut src = Cursor::new(&data[offset..]);
            let refs_indexes =
                Self::read_refs_indexes(&mut src, header.ref_size, cell_index, header.cells_count)?;

            if !header.roots_indexes.contains(&(cell_index as u32))
                && cell::store_hashes(&data[offset..])
            {
                flags |= BocFlags::IntHashes;
            }

            // let mut cell_data = CellData::with_external_data(&data, offset)?;

            let cell = if level_mask(&data[offset..]).mask() == 0 {
                // To calculate hash of this cell we need to know only
                // repr hashes of its children

                let mut ref_hashes = smallvec::SmallVec::<[UInt256; MAX_REFERENCES_COUNT]>::new();
                let mut ref_depths = smallvec::SmallVec::<[u16; MAX_REFERENCES_COUNT]>::new();
                for ref_cell_index in refs_indexes {
                    let (h, d) = cells_storage.load_hash_and_depth(ref_cell_index)?;
                    ref_hashes.push(h);
                    ref_depths.push(d);
                }
                Cell::with_data_and_loader(
                    &data[offset..],
                    true,
                    &ref_hashes,
                    &ref_depths,
                    cells_storage.loader(),
                    None,
                )?
            } else {
                // To calculate hash of this cell we need to know all hashes of its children,
                // so we need to load them from the storage

                let mut refs = smallvec::SmallVec::<[Cell; MAX_REFERENCES_COUNT]>::new();
                for ref_cell_index in refs_indexes {
                    let child = cells_storage.load_cell(ref_cell_index)?;
                    refs.push(child.clone());
                }
                Cell::with_data_and_refs(&data[offset..], true, &refs, Some(self.max_depth), None)?
            };
            cells_storage.store_cell(cell_index as u32, &cell)?;
            if !(self.load_cell_callback)(&cell) {
                interrupted = true;
                break;
            }
        }
        #[cfg(not(target_family = "wasm"))]
        let constructing_time = now1.elapsed().as_millis();

        let mut roots = Vec::new();
        if !interrupted {
            let mut all_roots_with_hashes = true;
            for i in &header.roots_indexes {
                check_abort(self.abort)?;
                let root = cells_storage.load_cell(*i)?;
                if !root.store_hashes() {
                    all_roots_with_hashes = false;
                }
                roots.push(root);
            }
            if all_roots_with_hashes {
                flags |= BocFlags::TopHash;
            }
        }

        #[cfg(not(target_family = "wasm"))]
        let now1 = std::time::Instant::now();
        if !interrupted && header.has_crc {
            let crc = crc32_digest(&data[..data.len() - 4]);
            src.set_position(data.len() as u64 - 4);
            let read_crc = src.read_le_u32()?;
            if read_crc != crc {
                fail!("crc not the same, values: {}, {}", read_crc, crc)
            }
        }
        #[cfg(not(target_family = "wasm"))]
        let crc_time = now1.elapsed().as_millis();

        #[cfg(not(target_family = "wasm"))]
        let now1 = std::time::Instant::now();
        cells_storage.cleanup()?;
        #[cfg(not(target_family = "wasm"))]
        let cleanup_time = now1.elapsed().as_millis();
        #[cfg(not(target_family = "wasm"))]
        {
            let total_time = now.elapsed().as_millis();
            log::trace!(
                "TIME read_inmem: {}ms (index: {}, creating cells: {}, crc: {}, cleanup: {})",
                total_time,
                index_time,
                constructing_time,
                crc_time,
                cleanup_time
            );
        }

        Ok(BocReaderResult { roots, header, flags, interrupted })
    }

    fn collect_roots(
        header: &BocHeader,
        cells: &[Cell],
        flags: &mut BocFlags,
        interrupted: bool,
    ) -> Result<Vec<Cell>> {
        if interrupted {
            return Ok(Vec::new());
        }
        let mut roots = Vec::with_capacity(header.roots_indexes.len());
        let mut all_roots_with_hashes = true;
        for &idx in &header.roots_indexes {
            let rev = header.cells_count - 1 - idx as usize;
            let root = cells.get(rev).ok_or_else(|| error!("Cell #{} was not found", rev))?;
            if !root.store_hashes() {
                all_roots_with_hashes = false;
            }
            roots.push(root.clone());
        }
        if all_roots_with_hashes {
            *flags |= BocFlags::TopHash;
        }
        Ok(roots)
    }

    fn read_index(
        &self,
        data: &'a [u8],
        cursor: &mut Cursor<&[u8]>,
        header: &BocHeader,
    ) -> Result<BocIndex<'a>> {
        if !header.index_included {
            let mut index = Vec::with_capacity(header.cells_count);
            for _ in 0_usize..header.cells_count {
                check_abort(self.abort)?;
                index.push(cursor.position());
                Self::skip_cell(cursor, header.ref_size)?;
            }
            Ok(BocIndex::Own(index))
        } else {
            let index = cursor.position();
            let cells_start = index + header.cells_count as u64 * header.offset_size as u64;
            Ok(BocIndex::Ref(&data[index as usize..], cells_start))
        }
    }

    // Reads only boc header and root cell data, without references and root hashes resolving.
    // The function doesn't check boc correctness and integrity!
    pub fn read_root(&mut self, data: &[u8]) -> Result<(BocHeader, SliceData)> {
        let mut src = Cursor::new(data);

        // Read header
        let (header, _) = self.read_header(&mut src)?;
        Self::precheck_cells_tree_len(&header, src.position(), data.len() as u64, false)?;
        if header.roots_count != 1 {
            fail!("Invalid boc: expected 1 root, found {}", header.roots_count);
        }

        // Deteremine root index
        let root_index = if header.magic == BOC_GENERIC_TAG {
            header.roots_indexes[0] as usize
        } else {
            0_usize
        };

        // Determine root cell offset
        let mut offset = 0;
        if header.index_included {
            let index = &data[src.position() as usize..];
            let index_size = header
                .cells_count
                .checked_mul(header.offset_size)
                .ok_or_else(|| error!("index size overflow"))?;
            if index.len() < index_size {
                fail!("Invalid data: too small to fit index");
            }
            offset = src.position() as usize
                + header
                    .cells_count
                    .checked_mul(header.offset_size)
                    .ok_or_else(|| error!("index size overflow"))?;
            if root_index > 0 {
                let o = (root_index - 1) * header.offset_size;
                let mut o2 = Cursor::new(&index[o..o + header.offset_size])
                    .read_be_uint(header.offset_size)? as usize;
                if header.has_cache_bits {
                    o2 >>= 1;
                }
                offset = offset.checked_add(o2).ok_or_else(|| error!("cell offset overflow"))?;
            }
        } else {
            for cell_index in 0_usize..header.cells_count {
                if cell_index == root_index {
                    offset = src.position() as usize;
                    break;
                }
                Self::skip_cell(&mut src, header.ref_size)?;
            }
        }
        if offset == 0 {
            fail!("Can't found root cell offset");
        } else if offset >= data.len() {
            fail!("Data is too short or index {} is invalid", root_index);
        }

        // Read root cell data
        let cell_data = Cell::check_data(&data[offset..], true)?;
        let slice = SliceData::with_bitstring(cell_data.data, cell_data.bit_len);

        Ok((header, slice))
    }

    pub fn read_header<T>(&self, src: &mut T) -> Result<(BocHeader, BocFlags)>
    where
        T: Read,
    {
        let magic = src.read_be_u32()?;
        let first_byte = src.read_byte()?;
        let index_included;
        let mut has_crc = false;
        let ref_size;
        let mut has_cache_bits = false;

        match magic {
            BOC_INDEXED_TAG => {
                ref_size = first_byte as usize;
                index_included = true;
            }
            BOC_INDEXED_CRC32_TAG => {
                ref_size = first_byte as usize;
                index_included = true;
                has_crc = true;
            }
            BOC_GENERIC_TAG => {
                index_included = first_byte & 0b1000_0000 != 0;
                has_crc = first_byte & 0b0100_0000 != 0;
                has_cache_bits = first_byte & 0b0010_0000 != 0;
                let flags = (first_byte & 0b0001_1000) >> 3;
                if flags != 0 {
                    fail!("non-zero flags field is not supported")
                }
                ref_size = (first_byte & 0b0000_0111) as usize;
            }
            _ => fail!("unknown BOC_TAG: {}", magic),
        };

        if has_cache_bits && !index_included {
            fail!("invalid header")
        }

        if ref_size == 0 || ref_size > 4 {
            fail!("ref size has to be more than 0 and less or equal 4, actual value: {}", ref_size)
        }

        let offset_size = src.read_byte()? as usize;
        if offset_size == 0 || offset_size > 8 {
            fail!("offset size has to be less or equal 8, actual value: {}", offset_size)
        }

        let cells_count = src.read_be_uint(ref_size)? as usize; // cells:(##(size * 8))
        let roots_count = src.read_be_uint(ref_size)? as usize; // roots:(##(size * 8))
        let absent_count = src.read_be_uint(ref_size)? as usize; // absent:(##(size * 8)) { roots + absent <= cells }

        if cells_count == 0 {
            fail!("cell count is zero")
        }
        if roots_count == 0 {
            fail!("root cell count is zero")
        }
        if roots_count > MAX_ROOTS_COUNT {
            fail!("too many roots")
        }
        if (magic == BOC_INDEXED_TAG || magic == BOC_INDEXED_CRC32_TAG) && roots_count > 1 {
            fail!(
                "roots count has to be less or equal 1 for TAG: {}, value: {}",
                magic,
                offset_size
            )
        }
        if roots_count + absent_count > cells_count {
            fail!(
                "roots count + absent count has to be less or equal than cells count, roots: {}, \
                absent: {}, cells: {}",
                roots_count,
                absent_count,
                cells_count
            );
        }
        if absent_count != 0 {
            fail!("absent cells are not supported")
        }

        let tot_cells_size = src.read_be_uint(offset_size)? as usize; // tot_cells_size:(##(off_bytes * 8))
        let max_cell_size = 2 + // descr bytes
            4 * (DEPTH_SIZE + SHA256_SIZE) + // stored hashe & depths
            MAX_DATA_BYTES +
            MAX_REFERENCES_COUNT * ref_size;
        let min_cell_size = 2; // descr bytes only
                               // every raw cell except roots must be referenced at least once, hence the formula
        let tot_cells_size_minimal =
            cells_count * (min_cell_size + ref_size) - ref_size * roots_count;
        if tot_cells_size < tot_cells_size_minimal {
            fail!("tot_cells_size is too small with respect to cells_count");
        }
        if tot_cells_size > max_cell_size * cells_count {
            fail!("tot_cells_size is too big with respect to cells_count");
        }

        let roots_indexes = if magic == BOC_GENERIC_TAG {
            // root_list:(roots * ##(size * 8))
            let mut roots_indexes = Vec::with_capacity(roots_count);
            for _ in 0..roots_count {
                let index = src.read_be_uint(ref_size)? as u32;
                if index as usize >= cells_count {
                    fail!(
                        "Invalid root index {} (greater than cells count {})",
                        index,
                        cells_count
                    );
                }
                if roots_indexes.contains(&index) {
                    fail!("Duplicate root index {} found", index);
                }
                roots_indexes.push(index); // cells:(##(size * 8))
            }
            roots_indexes
        } else {
            vec![0]
        };

        let header = BocHeader {
            magic,
            roots_count,
            ref_size,
            index_included,
            cells_count,
            offset_size,
            has_crc,
            has_cache_bits,
            roots_indexes,
            tot_cells_size,
        };
        let mut flags = BocFlags::None;
        if header.index_included {
            flags |= BocFlags::Index;
        }
        if header.has_crc {
            flags |= BocFlags::Crc32;
        }
        if header.has_cache_bits {
            flags |= BocFlags::CacheBits;
        }

        Ok((header, flags))
    }

    /// Pushes cell to the list and calls the load callback.
    /// Returns `false` if loading was interrupted by the callback.
    fn push_cell(&self, cell: Cell, cells: &mut Vec<Cell>) -> bool {
        let should_continue = (self.load_cell_callback)(&cell);
        cells.push(cell);
        should_continue
    }

    fn precheck_cells_tree_len(
        header: &BocHeader,
        header_len: u64,
        actual_len: u64,
        unbounded: bool,
    ) -> Result<()> {
        // calculate boc len
        let index_size =
            header.index_included as u64 * (header.cells_count as u64 * header.offset_size as u64);
        let len =
            header_len + index_size + header.tot_cells_size as u64 + header.has_crc as u64 * 4;
        if unbounded {
            if actual_len < len {
                fail!("Actual boc length {} is smaller than calculated one {}", actual_len, len);
            }
        } else if actual_len != len {
            fail!("Actual boc length {} in not equal calculated one {}", actual_len, len);
        }
        Ok(())
    }

    fn read_refs_indexes<T>(
        src: &mut T,
        ref_size: usize,
        cell_index: usize,
        cells_count: usize,
    ) -> Result<smallvec::SmallVec<[u32; 4]>>
    where
        T: Read + Seek,
    {
        let mut d1d2 = [0_u8; 2];
        src.read_exact(&mut d1d2[0..2])?;
        let to_skip = cell::full_len(&d1d2) - 2;
        src.seek(SeekFrom::Current(to_skip as i64))?;

        let refs_count = cell::refs_count(&d1d2);
        let mut references: smallvec::SmallVec<[u32; 4]> =
            smallvec::SmallVec::with_capacity(refs_count);
        for _ in 0..refs_count {
            let i = src.read_be_uint(ref_size)? as usize;
            if i >= cells_count || i <= cell_index {
                fail!(
                    "reference out of range, cells_count: {}, ref: {}, cell_index: {}",
                    cells_count,
                    i,
                    cell_index
                )
            } else {
                references.push(i as u32);
            }
        }

        Ok(references)
    }

    fn skip_cell<T>(src: &mut T, ref_size: usize) -> Result<()>
    where
        T: Read + Seek,
    {
        let mut d1d2 = [0_u8; 2];
        src.read_exact(&mut d1d2[0..2])?;
        let rest_size = cell::full_len(&d1d2) + ref_size * cell::refs_count(&d1d2) - 2;
        src.seek(SeekFrom::Current(rest_size as i64))?;
        Ok(())
    }
}

/// Wraps I/O operations and computes CRC32-C of the data being processed.
/// Both reader and writer paths buffer data internally so that CRC is computed
/// in large chunks instead of on every tiny read/write call.
/// A single instance is used either for reading or for writing, never both.
const CRC_BUF_SIZE: usize = 32 * 1024;

struct IoCrcFilter<'a, T> {
    io_object: &'a mut T,
    hasher: Crc32,
    buf: Vec<u8>,
    buf_pos: usize,
}

impl<'a, T: Write> IoCrcFilter<'a, T> {
    pub fn new_writer(io_object: &'a mut T) -> Self {
        IoCrcFilter {
            io_object,
            hasher: Crc32::new(),
            buf: Vec::with_capacity(CRC_BUF_SIZE),
            buf_pos: 0,
        }
    }

    fn flush_buf(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            self.hasher.update(&self.buf);
            self.io_object.write_all(&self.buf)?;
            self.buf.clear();
        }
        Ok(())
    }

    pub fn finalize(mut self) -> Result<()> {
        self.flush_buf()?;
        let crc = self.hasher.finalize();
        self.io_object.write_all(&crc.to_le_bytes())?;
        Ok(())
    }
}

impl<'a, T: Read> IoCrcFilter<'a, T> {
    pub fn new_reader(io_object: &'a mut T) -> Self {
        IoCrcFilter { io_object, hasher: Crc32::new(), buf: Vec::new(), buf_pos: 0 }
    }

    pub fn check_crc(mut self) -> Result<()> {
        // Hash the consumed portion of the current buffer
        if self.buf_pos > 0 {
            self.hasher.update(&self.buf[..self.buf_pos]);
        }
        let crc = self.hasher.finalize();

        // Read stored CRC — it may be partially or fully in the read-ahead buffer
        let remaining = self.buf.len() - self.buf_pos;
        let mut crc_bytes = [0u8; 4];
        let from_buf = remaining.min(4);
        crc_bytes[..from_buf].copy_from_slice(&self.buf[self.buf_pos..self.buf_pos + from_buf]);
        if from_buf < 4 {
            self.io_object.read_exact(&mut crc_bytes[from_buf..])?;
        }
        let read_crc = u32::from_le_bytes(crc_bytes);

        if read_crc != crc {
            fail!("crc not the same, values: {}, {}", read_crc, crc)
        }
        Ok(())
    }
}

impl<T: Seek> Seek for IoCrcFilter<'_, T> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match pos {
            SeekFrom::Current(0) => {
                let p = self.io_object.stream_position()?;
                let buffered = (self.buf.len() - self.buf_pos) as u64;
                Ok(p - buffered)
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "IoCrcFilter does not support seeking",
            )),
        }
    }
}

impl<T> Write for IoCrcFilter<'_, T>
where
    T: Write,
{
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buf.extend_from_slice(buf);
        if self.buf.len() >= CRC_BUF_SIZE {
            self.flush_buf()?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.flush_buf()?;
        self.io_object.flush()
    }
}

impl<T> Read for IoCrcFilter<'_, T>
where
    T: Read,
{
    fn read(&mut self, dest: &mut [u8]) -> std::io::Result<usize> {
        if self.buf_pos >= self.buf.len() {
            // Hash the fully consumed buffer before refilling
            if !self.buf.is_empty() {
                self.hasher.update(&self.buf);
            }
            self.buf.resize(CRC_BUF_SIZE, 0);
            let n = self.io_object.read(&mut self.buf)?;
            self.buf.truncate(n);
            self.buf_pos = 0;
            if n == 0 {
                return Ok(0);
            }
        }
        let available = self.buf.len() - self.buf_pos;
        let n = dest.len().min(available);
        dest[..n].copy_from_slice(&self.buf[self.buf_pos..self.buf_pos + n]);
        self.buf_pos += n;
        Ok(n)
    }
}

#[cfg(test)]
#[path = "tests/test_boc.rs"]
mod tests;
