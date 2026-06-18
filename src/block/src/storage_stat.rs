/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{
    define_HashmapE, dictionary::FixedBitsKey, error, fail, AccountStorage, BuilderData, Cell,
    Deserializable, HashmapType, IBitstring, Result, Serializable, SliceData, StateInit,
    StorageUsed, UInt256,
};
use smallvec::SmallVec;

#[cfg(test)]
#[path = "tests/test_storage_stat.rs"]
mod tests;

const DICT_PROOF_TAG: u32 = 0x37c1e3fc;
const CONSENSUS_EXTRA_DATA_TAG: u32 = 0x638eb292;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct StorageStatCellInfo {
    pub ref_count: u32,
    pub max_merkle_depth: u8,
    // not serialized
    pub ref_count_diff: i32,
}

impl Serializable for StorageStatCellInfo {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u32(self.ref_count)?;
        cell.append_bits(self.max_merkle_depth as usize, 2)?;
        Ok(())
    }
}
impl Deserializable for StorageStatCellInfo {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        self.ref_count = cell.get_next_u32()?;
        self.max_merkle_depth = cell.get_next_int(2)? as u8;
        self.ref_count_diff = 0;
        Ok(())
    }
}

define_HashmapE!(StorageStatDict, 256, StorageStatCellInfo);
pub type StorageRoots = SmallVec<[Cell; 3]>;

/// Per-account storage statistics over the `code`/`data`/`library` subtrees of `StateInit`.
/// Counts total cells/bits (excluding the `AccountStorage` root itself) and tracks per-cell
/// refcount + merkle depth. The dictionary form is serializable, its repr_hash lands in
/// `StorageInfo.storage_extra.dict_hash` and is used by other nodes to skip a full rebuild.
///
/// Two storage layers for the same data: `cache` is a fast in-memory AHashMap (L1, holds pending
/// diffs and acts as a hot cache over the dict); `dict` is the slow but serializable HashmapE
/// (L2, the committed state). Reads consult `cache` first, fall back to `dict`. Writes
/// (`add_cell`/`remove_cell`) only touch `cache`; `calc_dict()` is the single point that flushes
/// `cache` into `dict` via hashmap_multiset. After a flush `cache` is NOT cleared — the entries
/// are kept (with `ref_count_diff = 0`) as a hot L1 over `dict` for subsequent operations.
///
/// Three meaningful initial states from constructors:
/// - empty: `Default` / `empty()` / `new(_, used)` with `used.cells == 0` — everything zero.
/// - seeded: `new(storage, used)` with non-empty used — totals trusted from `used`, but `dict`
///   and `cache` are empty. Per-cell data must be rebuilt on first `calc_dict()` via
///   `fill_cache_from_roots()` (O(N) walk), unless the dict is imported externally first.
/// - stored: `try_from_dict(dict, ...)` — `dict` is populated from a known dict-cell, `cache`
///   is empty. This is the cheap path; used by `Account::import_storage_stat_dict` when the
///   engine-level LRU (`Engine::storage_dicts_cache`) has the dict for this `dict_hash`.
///
/// Invariants: `roots` mirrors `get_roots(storage.state_init())`; root changes go through
/// `replace_roots`, which produces an incremental diff via `add_cell`/`remove_cell`. The
/// `(dict empty && cache empty)` combination is treated as "seeded or freshly empty" and lets
/// `replace_roots` reset roots/totals before rebuilding incrementally.
#[derive(Default, Clone, PartialEq)]
pub struct AccountStorageStat {
    dict: StorageStatDict,
    roots: StorageRoots,
    total_cells: u64,
    total_bits: u64,
    cache: ahash::AHashMap<UInt256, StorageStatCellInfo>,
    dict_updated: bool,
}

impl std::fmt::Debug for AccountStorageStat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut builder = f.debug_struct("AccountStorageStat");
        builder.field("dict", &self.dict);
        builder.field("total_cells", &self.total_cells);
        builder.field("total_bits", &self.total_bits);
        builder.field("dict_updated", &self.dict_updated);
        builder.finish()
    }
}

impl AccountStorageStat {
    pub fn new(storage: &AccountStorage, used: &StorageUsed) -> Self {
        if used.cells() == 0 {
            return Self::empty();
        }
        let Ok(storage_cell) = storage.write_to_new_cell() else {
            return Self::empty();
        };
        let storage_root_bits = storage_cell.length_in_bits() as u64;
        let (Some(total_cells), Some(total_bits)) =
            (used.cells().checked_sub(1), used.bits().checked_sub(storage_root_bits))
        else {
            return Self::empty();
        };
        Self {
            dict: StorageStatDict::new(),
            roots: Self::get_roots(storage.state_init()),
            total_cells,
            total_bits,
            cache: Default::default(),
            dict_updated: false,
        }
    }

    fn empty() -> Self {
        Self {
            dict: StorageStatDict::new(),
            roots: StorageRoots::new(),
            total_cells: 0,
            total_bits: 0,
            cache: Default::default(),
            dict_updated: true,
        }
    }

    pub fn try_from_dict(dict: Cell, storage: &AccountStorage, used: &StorageUsed) -> Result<Self> {
        let storage_cell = storage.write_to_new_cell()?;
        let total_bits = used.bits().checked_sub(storage_cell.length_in_bits() as u64);
        let total_cells = used.cells().checked_sub(1);
        let (Some(total_cells), Some(total_bits)) = (total_cells, total_bits) else {
            fail!(
                "StorageUsed is too small (cells {}, bits {}, storage root bits {}), \
                cannot create AccountStorageStat",
                used.cells(),
                used.bits(),
                storage_cell.length_in_bits()
            )
        };
        Ok(Self {
            dict: StorageStatDict::with_hashmap(Some(dict)),
            roots: Self::get_roots(storage.state_init()),
            total_cells,
            total_bits,
            cache: Default::default(),
            dict_updated: false,
        })
    }

    fn fill_cache_from_roots(&mut self) -> Result<()> {
        let saved_cells = std::mem::replace(&mut self.total_cells, 0);
        let saved_bits = std::mem::replace(&mut self.total_bits, 0);
        for root in self.roots.clone() {
            self.add_cell(&root)?;
        }
        self.total_cells = saved_cells;
        self.total_bits = saved_bits;
        Ok(())
    }

    pub fn calc_dict(&mut self) -> Result<Option<&Cell>> {
        if !self.dict_updated {
            // Need to fill cache if we only have initial data - no roots changed
            if self.cache.is_empty() && self.dict.is_empty() && !self.roots.is_empty() {
                self.fill_cache_from_roots()?;
            }

            fn map_entry<'a>(
                (hash, data): (&'a UInt256, &mut StorageStatCellInfo),
            ) -> (FixedBitsKey<'a>, Option<SliceData>) {
                data.ref_count_diff = 0;
                let key = FixedBitsKey::new(hash.as_slice());
                if data.ref_count == 0 {
                    (key, None)
                } else {
                    (key, data.write_to_bitstring().ok())
                }
            }

            if self.dict.is_empty() {
                // no filter when dict is filled from scratch - `collect` will use size hint for allocation
                self.dict.0.hashmap_multiset(self.cache.iter_mut().map(map_entry))?;
            } else {
                self.dict.0.hashmap_multiset(
                    self.cache
                        .iter_mut()
                        .filter(|(_, data)| data.ref_count_diff != 0)
                        .map(map_entry),
                )?;
            }

            self.dict_updated = true;
        }

        Ok(self.dict.root())
    }

    pub fn calc_stat(&mut self, storage: &AccountStorage) -> Result<StorageUsed> {
        self.replace_roots(Self::get_roots(storage.state_init()))?;

        let cell = storage.serialize()?;
        StorageUsed::with_values_checked(
            self.total_cells + 1,
            self.total_bits + cell.bit_length() as u64,
        )
    }

    pub fn get_roots(storage: Option<&StateInit>) -> StorageRoots {
        match storage {
            Some(state_init) => {
                // storage root and currency collection are not counted in stats
                let mut roots = StorageRoots::new();
                if let Some(code) = state_init.code() {
                    roots.push(code.clone());
                }
                if let Some(data) = state_init.data() {
                    roots.push(data.clone());
                }
                if let Some(lib) = state_init.library.root() {
                    roots.push(lib.clone());
                }
                roots
            }
            None => StorageRoots::new(),
        }
    }

    fn replace_roots(&mut self, roots: StorageRoots) -> Result<()> {
        if roots == self.roots {
            return Ok(());
        }

        // Seeded/empty stat: no per-cell info to update incrementally, and a partial add can't
        // dedup against the uncounted seeded cells (would double-count shared ones). Rebuild fully
        // from the new roots. Invariant: with an empty dict a non-empty cache is therefore always
        // complete, so removals resolve from it without a dict (dict-less accounts stay incremental).
        if self.dict.is_empty() && self.cache.is_empty() {
            self.roots.clear();
            self.total_cells = 0;
            self.total_bits = 0;
        }

        self.dict_updated = false;
        for root in &roots {
            if !self.roots.contains(root) {
                self.add_cell(root)?;
            }
        }
        for root in &std::mem::take(&mut self.roots) {
            if !roots.contains(root) {
                self.remove_cell(root)?;
            }
        }
        self.roots = roots;
        Ok(())
    }

    fn add_cell(&mut self, cell: &Cell) -> Result<u8> {
        let hash = cell.repr_hash().clone();
        let mut max_merkle_depth = 0;
        if let Some(data) = self.cache.get_mut(&hash) {
            data.ref_count += 1;
            data.ref_count_diff += 1;
            max_merkle_depth = data.max_merkle_depth;

            if data.ref_count == 1 {
                for i in 0..cell.references_count() {
                    self.add_cell(&cell.reference_without_usage(i)?)?;
                }
                self.total_cells += 1;
                self.total_bits += cell.bit_length() as u64;
            }
        } else if let Some(data) = self.dict.get(&hash)? {
            max_merkle_depth = data.max_merkle_depth;
            self.cache.insert(
                hash,
                StorageStatCellInfo {
                    ref_count: data.ref_count + 1,
                    max_merkle_depth,
                    ref_count_diff: 1,
                },
            );
        } else {
            for i in 0..cell.references_count() {
                let child_depth = self.add_cell(&cell.reference_without_usage(i)?)?;
                max_merkle_depth = max_merkle_depth.max(child_depth);
            }
            if cell.is_merkle() {
                max_merkle_depth += 1;
            }
            self.total_cells += 1;
            self.total_bits += cell.bit_length() as u64;
            let data = StorageStatCellInfo { ref_count: 1, max_merkle_depth, ref_count_diff: 1 };
            self.cache.insert(hash, data);
        }
        Ok(max_merkle_depth)
    }

    fn remove_cell(&mut self, cell: &Cell) -> Result<()> {
        let hash = cell.repr_hash().clone();
        let removed = if let Some(data) = self.cache.get_mut(&hash) {
            data.ref_count -= 1;
            data.ref_count_diff -= 1;
            data.ref_count == 0
        } else {
            let data = self.dict.get(&hash)?.ok_or_else(|| {
                error!("Cell with hash {} not found in storage stat dictionary", hash)
            })?;
            self.cache.insert(
                hash,
                StorageStatCellInfo {
                    ref_count: data.ref_count - 1,
                    max_merkle_depth: data.max_merkle_depth,
                    ref_count_diff: -1,
                },
            );
            data.ref_count == 1
        };

        if removed {
            for i in 0..cell.references_count() {
                self.remove_cell(&cell.reference_without_usage(i)?)?;
            }
            self.total_cells -= 1;
            self.total_bits -= cell.bit_length() as u64;
        }

        Ok(())
    }

    pub fn total_cells(&self) -> u64 {
        self.total_cells
    }

    pub fn total_bits(&self) -> u64 {
        self.total_bits
    }

    pub fn max_merkle_depth(&self) -> Result<u8> {
        let mut result = 0;
        for root in &self.roots {
            let depth = if let Some(data) = self.cache.get(root.repr_hash()) {
                data.max_merkle_depth
            } else {
                self.dict
                    .get(root.repr_hash())?
                    .ok_or_else(|| {
                        error!("Root {} not found in storage stat dictionary", root.repr_hash())
                    })?
                    .max_merkle_depth
            };
            result = result.max(depth);
        }
        Ok(result)
    }

    pub fn is_changed(&self) -> bool {
        !self.cache.is_empty()
    }
}

#[derive(Debug, Default, Clone)]
pub struct AccountStorageDictProof {
    pub proof: Cell,
}

impl Serializable for AccountStorageDictProof {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u32(DICT_PROOF_TAG)?;
        cell.checked_append_reference(self.proof.clone())?;
        Ok(())
    }
}

impl Deserializable for AccountStorageDictProof {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_u32()?;
        if tag != DICT_PROOF_TAG {
            fail!(
                "Invalid AccountStorageDictProof tag: expected {:#x}, found {:#x}",
                DICT_PROOF_TAG,
                tag
            );
        }
        self.proof = cell.checked_drain_reference()?;
        Ok(())
    }
}

/// consensus_extra_data#638eb292 flags:# gen_utime_ms:uint64 = ConsensusExtraData;
#[derive(Debug, Default, Clone)]
pub struct ConsensusExtraData {
    pub flags: u32,
    pub gen_utime_ms: u64,
}

impl ConsensusExtraData {
    pub const TAG: u32 = CONSENSUS_EXTRA_DATA_TAG;
}

impl Serializable for ConsensusExtraData {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        cell.append_u32(CONSENSUS_EXTRA_DATA_TAG)?;
        cell.append_u32(self.flags)?;
        cell.append_u64(self.gen_utime_ms)?;
        Ok(())
    }
}

impl Deserializable for ConsensusExtraData {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        let tag = cell.get_next_u32()?;
        if tag != CONSENSUS_EXTRA_DATA_TAG {
            fail!(
                "Invalid ConsensusExtraData tag: expected {:#x}, found {:#x}",
                CONSENSUS_EXTRA_DATA_TAG,
                tag
            );
        }
        self.flags = cell.get_next_u32()?;
        self.gen_utime_ms = cell.get_next_u64()?;
        Ok(())
    }
}
