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
use crate::{
    error,
    error::{BlockError, Result},
    fail, BuilderData, Cell, CellType, Deserializable, IBitstring, LevelMask, MerkleProof,
    Serializable, SliceData, UInt256,
};
use std::{
    fmt::{Display, Formatter},
    sync::Arc,
};

#[cfg(test)]
#[path = "tests/test_merkle_update.rs"]
mod tests;

#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct MerkleUpdateApplyMetrics {
    pub old_cells: usize,
    pub old_pruned: usize,
    pub new_cells: usize,
    pub new_pruned: usize,
    pub created_new_cells: usize,
}

impl Display for MerkleUpdateApplyMetrics {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(
            f,
            "\
            old_cells:        {:>10}\n\
            old_pruned:       {:>10}\n\
            new_cells:        {:>10}\n\
            new_pruned:       {:>10}\n\
            created_new_cells:{:>10}\n",
            self.old_cells,
            self.old_pruned,
            self.new_cells,
            self.new_pruned,
            self.created_new_cells,
        )
    }
}

pub trait CellsFactory: Send + Sync {
    fn create_cell(self: Arc<Self>, builder: BuilderData) -> Result<Cell>;
    fn create_lazy_load_cell(self: Arc<Self>, pruned: &Cell, merkle_depth: u8) -> Result<Cell>;
}

pub struct DefaultCellsFactory;
impl CellsFactory for DefaultCellsFactory {
    fn create_cell(self: Arc<Self>, builder: BuilderData) -> Result<Cell> {
        builder.into_cell()
    }
    fn create_lazy_load_cell(self: Arc<Self>, _pruned: &Cell, _merkle_depth: u8) -> Result<Cell> {
        fail!("Lazy load cells are not supported by default factory")
    }
}

/*
!merkle_update {X:Type} old_hash:uint256 new_hash:uint256
old:^X new:^X = MERKLE_UPDATE X;
*/
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MerkleUpdate {
    pub old_hash: UInt256,
    pub new_hash: UInt256,
    pub old_depth: u16,
    pub new_depth: u16,
    pub old: Cell, // reference
    pub new: Cell, // reference
}

impl Default for MerkleUpdate {
    fn default() -> MerkleUpdate {
        let old = Cell::default();
        let new = Cell::default();
        MerkleUpdate {
            old_hash: Cell::hash(&old, 0).clone(),
            new_hash: Cell::hash(&new, 0).clone(),
            old_depth: 0,
            new_depth: 0,
            old,
            new,
        }
    }
}

impl Display for MerkleUpdate {
    fn fmt(&self, f: &mut Formatter) -> std::fmt::Result {
        write!(
            f,
            "MerkleUpdate (\
            old_hash: {:x},\
            new_hash: {:x},\
            old_depth: {},\
            new_depth: {},\
            old: {:#.2},\
            new: {:#.2}\
        )",
            self.old_hash, self.new_hash, self.old_depth, self.new_depth, self.old, self.new
        )
    }
}

impl Deserializable for MerkleUpdate {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        if cell.pos() != 0 {
            fail!("Merkle update have to fill full cell from its zeroth bit.")
        }
        if CellType::try_from(cell.get_next_byte()?)? != CellType::MerkleUpdate {
            fail!(BlockError::InvalidData("invalid Merkle update root's cell type".to_string()))
        }
        self.old_hash.read_from(cell)?;
        self.new_hash.read_from(cell)?;
        self.old_depth = cell.get_next_u16()?;
        self.new_depth = cell.get_next_u16()?;
        self.old = cell.checked_drain_reference()?;
        self.new = cell.checked_drain_reference()?;

        if self.old_hash != *Cell::hash(&self.old, 0) {
            fail!(BlockError::WrongMerkleUpdate(
                "Stored old hash is not equal calculated one".to_string()
            ))
        }
        if self.new_hash != *Cell::hash(&self.new, 0) {
            fail!(BlockError::WrongMerkleUpdate(
                "Stored new hash is not equal calculated one".to_string()
            ))
        }
        if self.old_depth != Cell::depth(&self.old, 0) {
            fail!(BlockError::WrongMerkleUpdate(
                "Stored old depth is not equal calculated one".to_string()
            ))
        }
        if self.new_depth != Cell::depth(&self.new, 0) {
            fail!(BlockError::WrongMerkleUpdate(
                "Stored new depth is not equal calculated one".to_string()
            ))
        }

        Ok(())
    }
}

impl Serializable for MerkleUpdate {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if !cell.is_empty() {
            fail!("Merkle update have to fill full cell from its zeroth bit.")
        }
        cell.set_type(CellType::MerkleUpdate);
        cell.append_u8(u8::from(CellType::MerkleUpdate))?;
        self.old_hash.write_to(cell)?;
        self.new_hash.write_to(cell)?;
        cell.append_u16(self.old_depth)?;
        cell.append_u16(self.new_depth)?;
        cell.checked_append_reference(self.old.clone())?;
        cell.checked_append_reference(self.new.clone())?;
        Ok(())
    }
}

impl MerkleUpdate {
    /// Creating of a Merkle update
    pub fn create(old: &Cell, new: &Cell) -> Result<MerkleUpdate> {
        if old.repr_hash() == new.repr_hash() {
            // if trees are the same
            let hash = old.repr_hash().clone();
            let pruned_branch_cell = Self::make_pruned_branch_cell(old, 0)?.into_cell()?;
            Ok(MerkleUpdate {
                old_hash: hash.clone(),
                new_hash: hash,
                old_depth: old.repr_depth(),
                new_depth: old.repr_depth(),
                old: pruned_branch_cell.clone(),
                new: pruned_branch_cell,
            })
        } else {
            // trees traversal and update creating;
            let new_cells = Self::collect_cells(new);
            let mut pruned_branches = ahash::AHashMap::new();

            let old_update_cell =
                match Self::traverse_old_on_create(old, &new_cells, &mut pruned_branches, 0)? {
                    Some(old_update_cell) => old_update_cell,
                    // Nothing from old tree were pruned, lets prune all tree!
                    None => Self::make_pruned_branch_cell(old, 0)?,
                };
            let new_update_cell = Self::traverse_new_on_create(new, &pruned_branches)?;

            Ok(MerkleUpdate {
                old_hash: old.repr_hash().clone(),
                new_hash: new.repr_hash().clone(),
                old_depth: old.repr_depth(),
                new_depth: new.repr_depth(),
                old: old_update_cell.into_cell()?,
                new: new_update_cell.into_cell()?,
            })
        }
    }

    pub fn create_fast(
        old: &Cell,
        new: &Cell,
        is_visited_old: impl Fn(&UInt256) -> bool,
    ) -> Result<MerkleUpdate> {
        if old.repr_hash() == new.repr_hash() {
            // if trees are the same
            let hash = old.repr_hash().clone();
            let pruned_branch_cell = Self::make_pruned_branch_cell(old, 0)?.into_cell()?;
            Ok(MerkleUpdate {
                old_hash: hash.clone(),
                new_hash: hash,
                old_depth: old.repr_depth(),
                new_depth: old.repr_depth(),
                old: pruned_branch_cell.clone(),
                new: pruned_branch_cell,
            })
        } else {
            let mut pruned_branches = Some(ahash::AHashSet::new());
            let mut done_cells = ahash::AHashMap::new();
            let new_update_cell = MerkleProof::create_raw(
                new,
                &|hash| !is_visited_old(hash),
                &|_| false,
                0,
                &mut pruned_branches,
                &mut done_cells,
            )?;
            let pruned_branches = pruned_branches.unwrap();

            let mut used_paths_cells = ahash::AHashSet::new();
            let mut visited = ahash::AHashSet::new();
            if Self::collect_used_paths_cells(
                old,
                &is_visited_old,
                &pruned_branches,
                &mut ahash::AHashSet::new(),
                &mut used_paths_cells,
                &mut visited,
            )? {
                used_paths_cells.insert(old.repr_hash().clone());
            }

            let mut done_cells = ahash::AHashMap::new();
            let old_update_cell = MerkleProof::create_raw(
                old,
                &|hash| used_paths_cells.contains(hash),
                &|_| false,
                0,
                &mut None,
                &mut done_cells,
            )?;

            Ok(MerkleUpdate {
                old_hash: old.repr_hash().clone(),
                new_hash: new.repr_hash().clone(),
                old_depth: old.repr_depth(),
                new_depth: new.repr_depth(),
                old: old_update_cell,
                new: new_update_cell,
            })
        }
    }

    fn collect_cells(cell: &Cell) -> ahash::AHashMap<UInt256, Cell> {
        fn walker(cell: &Cell, hash: UInt256, cells: &mut ahash::AHashMap<UInt256, Cell>) {
            cells.insert(hash, cell.clone());
            for i in 0..cell.references_count() {
                let child_hash = cell.reference(i).unwrap().repr_hash().clone();
                if !cells.contains_key(&child_hash) {
                    let child = cell.reference(i).unwrap();
                    walker(&child, child_hash, cells);
                }
            }
        }
        let mut cells = ahash::AHashMap::new();
        walker(cell, cell.repr_hash().clone(), &mut cells);
        cells
    }

    fn collect_used_paths_cells(
        cell: &Cell,
        is_visited_old: &impl Fn(&UInt256) -> bool,
        pruned_branches: &ahash::AHashSet<UInt256>,
        visited_pruned_branches: &mut ahash::AHashSet<UInt256>,
        used_paths_cells: &mut ahash::AHashSet<UInt256>,
        visited: &mut ahash::AHashSet<UInt256>,
    ) -> Result<bool> {
        let repr_hash = cell.repr_hash();

        if visited.contains(repr_hash) {
            return Ok(false);
        }
        visited.insert(repr_hash.clone());

        if used_paths_cells.contains(repr_hash) {
            return Ok(false);
        }

        let is_pruned = if pruned_branches.contains(repr_hash) {
            if visited_pruned_branches.contains(repr_hash) {
                return Ok(false);
            }
            visited_pruned_branches.insert(repr_hash.clone());
            true
        } else {
            false
        };

        let mut collect = false;
        if is_visited_old(repr_hash) {
            let refs = cell.clone_references()?;
            for r in refs.iter() {
                collect |= Self::collect_used_paths_cells(
                    r,
                    is_visited_old,
                    pruned_branches,
                    visited_pruned_branches,
                    used_paths_cells,
                    visited,
                )?;
            }
            if collect {
                used_paths_cells.insert(repr_hash.clone());
            }
        }
        Ok(collect | is_pruned)
    }

    pub fn apply_for(&self, old_root: &Cell) -> Result<(Cell, MerkleUpdateApplyMetrics)> {
        self.apply_with_factory(old_root, &(Arc::new(DefaultCellsFactory) as Arc<dyn CellsFactory>))
    }

    /// Applies update to given tree of cells by returning new updated one
    pub fn apply_with_factory(
        &self,
        old_root: &Cell,
        factory: &Arc<dyn CellsFactory>,
    ) -> Result<(Cell, MerkleUpdateApplyMetrics)> {
        let mut metrics = MerkleUpdateApplyMetrics::default();
        let old_cells_hashes = self.check(old_root, Some(&mut metrics))?;
        let mut old_cells = ahash::AHashMap::new();
        Self::collect_old_cells(
            old_root,
            &old_cells_hashes,
            &mut old_cells,
            &mut ahash::AHashSet::new(),
            0,
        )?;

        // cells for new bag
        if self.new_hash == self.old_hash {
            Ok((old_root.clone(), MerkleUpdateApplyMetrics::default()))
        } else {
            let loader = |hash: &UInt256| {
                old_cells
                    .get(hash)
                    .cloned()
                    .ok_or_else(|| error!("Can't load cell with hash {:x}", hash))
            };
            let mut new_cells = ahash::AHashMap::new();
            let new_root = self.traverse_on_apply(
                &self.new,
                Some(&loader),
                &mut new_cells,
                0,
                factory,
                &mut 0,
            )?;
            metrics.created_new_cells = new_cells.len();

            // constructed tree's hash have to coincide with self.new_hash
            if *new_root.repr_hash() != self.new_hash {
                fail!(BlockError::WrongMerkleUpdate("new bag's hash mismatch".to_string()))
            }

            Ok((new_root, metrics))
        }
    }

    pub fn apply_with_loader(
        &self,
        old_root: &Cell,
        factory: &Arc<dyn CellsFactory>,
        loader: &dyn Fn(&UInt256) -> Result<Cell>,
    ) -> Result<(Cell, MerkleUpdateApplyMetrics)> {
        let mut metrics = MerkleUpdateApplyMetrics::default();
        let _ = self.check(old_root, Some(&mut metrics))?;

        // cells for new bag
        if self.new_hash == self.old_hash {
            Ok((old_root.clone(), MerkleUpdateApplyMetrics::default()))
        } else {
            let mut new_cells = ahash::AHashMap::new();
            let new_root = self.traverse_on_apply(
                &self.new,
                Some(loader),
                &mut new_cells,
                0,
                factory,
                &mut 0,
            )?;
            metrics.created_new_cells = new_cells.len();

            // constructed tree's hash have to coincide with self.new_hash
            if *new_root.repr_hash() != self.new_hash {
                fail!(BlockError::WrongMerkleUpdate("new bag's hash mismatch".to_string()))
            }

            Ok((new_root, metrics))
        }
    }

    pub fn apply_lazy_unchecked(
        &self,
        factory: &Arc<dyn CellsFactory>,
    ) -> Result<(Cell, MerkleUpdateApplyMetrics)> {
        let mut metrics = MerkleUpdateApplyMetrics::default();

        // cells for new bag
        if self.new_hash == self.old_hash {
            let root = factory.clone().create_lazy_load_cell(&self.new, 0)?;
            Ok((root, metrics))
        } else {
            let mut new_cells = ahash::AHashMap::new();
            let mut pruned_cells = 0;
            let new_root = self.traverse_on_apply(
                &self.new,
                None,
                &mut new_cells,
                0,
                factory,
                &mut pruned_cells,
            )?;
            metrics.created_new_cells = new_cells.len();
            metrics.new_pruned = pruned_cells;

            // constructed tree's hash have to coincide with self.new_hash
            if *new_root.repr_hash() != self.new_hash {
                fail!(BlockError::WrongMerkleUpdate("new bag's hash mismatch".to_string()))
            }

            Ok((new_root, metrics))
        }
    }

    /// Check the update corresponds given bag.
    /// The function is called from `apply_for`
    fn check(
        &self,
        old_root: &Cell,
        mut metrics: Option<&mut MerkleUpdateApplyMetrics>,
    ) -> Result<ahash::AHashSet<UInt256>> {
        // check that hash of `old_tree` is equal old hash from `self`
        if self.old_hash != *old_root.repr_hash() {
            fail!(BlockError::WrongMerkleUpdate("old bag's hash mismatch".to_string()))
        }

        // traversal along `self.new` and check all pruned branches.
        // All new tree's pruned branches have to be contained in old one
        let mut known_cells = ahash::AHashSet::new();
        let mut visited = ahash::AHashSet::new();
        let mut pruned_cells_count = 0;
        Self::traverse_old_on_check(
            &self.old,
            &mut known_cells,
            &mut visited,
            0,
            &mut pruned_cells_count,
        )?;
        if let Some(metrics) = metrics.as_mut() {
            metrics.old_cells = known_cells.len() - pruned_cells_count;
            metrics.old_pruned = pruned_cells_count;
        }
        let mut new_cells_count = 0;
        let mut pruned_cells_count = 0;
        Self::traverse_new_on_check(
            &self.new,
            &known_cells,
            &mut ahash::AHashSet::new(),
            0,
            &mut new_cells_count,
            &mut pruned_cells_count,
        )?;
        if let Some(metrics) = metrics.as_mut() {
            metrics.new_cells = new_cells_count;
            metrics.new_pruned = pruned_cells_count;
        }
        Ok(known_cells)
    }

    /// Recursive traverse merkle update tree while merkle update applying
    /// `cell` ordinary cell from merkle update's new tree;
    /// `old_cells` cells from old bag of cells;
    #[allow(clippy::only_used_in_recursion)]
    fn traverse_on_apply(
        &self,
        update_cell: &Cell,
        loader: Option<&dyn Fn(&UInt256) -> Result<Cell>>,
        new_cells: &mut ahash::AHashMap<UInt256, Cell>,
        merkle_depth: u8,
        cells_factory: &Arc<dyn CellsFactory>,
        pruned_cells: &mut usize,
    ) -> Result<Cell> {
        // We will recursively construct new skeleton for new cells
        // and connect unchanged branches to it

        let mut new_cell = BuilderData::new();
        new_cell.set_type(update_cell.cell_type());

        let child_merkle_depth =
            if update_cell.is_merkle() { merkle_depth + 1 } else { merkle_depth };

        // traverse references
        let mut child_mask = LevelMask::with_mask(0);
        let refs = update_cell.clone_references()?;
        for update_child in refs.iter() {
            let new_child = match update_child.cell_type() {
                CellType::Ordinary
                | CellType::MerkleProof
                | CellType::MerkleUpdate
                | CellType::LibraryReference => {
                    let new_child_hash = update_child.hash(child_merkle_depth as usize).clone();
                    if let Some(c) = new_cells.get(&new_child_hash) {
                        c.clone()
                    } else {
                        let c = self.traverse_on_apply(
                            update_child,
                            loader,
                            new_cells,
                            child_merkle_depth,
                            cells_factory,
                            pruned_cells,
                        )?;
                        new_cells.insert(new_child_hash, c.clone());
                        c
                    }
                }
                CellType::PrunedBranch => {
                    // if this pruned branch is related to current update
                    let mask = update_child.level_mask().mask();
                    *pruned_cells += 1;
                    if mask & (1 << child_merkle_depth) != 0 {
                        // connect branch from old bag instead pruned
                        let new_child_hash =
                            Cell::hash(update_child, update_child.level() as usize - 1);
                        if let Some(loader) = loader {
                            loader(new_child_hash)?
                        } else {
                            cells_factory
                                .clone()
                                .create_lazy_load_cell(update_child, child_merkle_depth)?
                        }
                    } else {
                        // else - just copy this cell (like an ordinary)
                        cells_factory.clone().create_cell(BuilderData::from_cell(update_child)?)?
                    }
                }
                _ => fail!("Unknown cell type while applying merkle update!"),
            };
            child_mask |= new_child.level_mask();
            new_cell.checked_append_reference(new_child)?;
        }

        // Copy data from update to constructed cell
        new_cell.append_bytestring(&SliceData::load_cell_ref(update_cell)?)?;

        cells_factory.clone().create_cell(new_cell)
    }

    fn traverse_new_on_create(
        new_cell: &Cell,
        common_pruned: &ahash::AHashMap<UInt256, Cell>,
    ) -> Result<BuilderData> {
        let mut new_update_cell = BuilderData::new();
        new_update_cell.set_type(new_cell.cell_type());
        let mut level_mask = new_cell.level_mask();
        let refs = new_cell.clone_references()?;
        for child in refs.iter() {
            let update_child = if let Some(pruned) = common_pruned.get(child.repr_hash()) {
                pruned.clone()
            } else {
                Self::traverse_new_on_create(child, common_pruned)?.into_cell()?
            };
            level_mask |= update_child.level_mask();
            new_update_cell.checked_append_reference(update_child)?;
        }

        new_update_cell.append_bytestring(&SliceData::load_cell_ref(new_cell)?)?;

        Ok(new_update_cell)
    }

    // If old_cell's child contains in new_cells - it transformed to pruned branch cell,
    //   else - recursion call for the child.
    // If any child is pruned branch (or contains pruned branch among their subtree)
    //   - all other skipped childs are transformed to pruned branches
    //   else - skip this cell (return None)
    fn traverse_old_on_create(
        old_cell: &Cell,
        new_cells: &ahash::AHashMap<UInt256, Cell>,
        pruned_branches: &mut ahash::AHashMap<UInt256, Cell>,
        mut merkle_depth: u8,
    ) -> Result<Option<BuilderData>> {
        if old_cell.is_merkle() {
            merkle_depth += 1;
        }

        let mut childs = vec![None; old_cell.references_count()];
        let mut has_pruned = false;

        let refs = old_cell.clone_references()?;
        for (i, child) in refs.iter().enumerate() {
            let child_hash = child.repr_hash();
            if let Some(common_cell) = new_cells.get(child_hash) {
                let pruned_branch_cell = Self::make_pruned_branch_cell(common_cell, merkle_depth)?;
                pruned_branches.insert(child_hash.clone(), pruned_branch_cell.clone().into_cell()?);

                childs[i] = Some(pruned_branch_cell);
                has_pruned = true;
            } else {
                childs[i] =
                    Self::traverse_old_on_create(child, new_cells, pruned_branches, merkle_depth)?;
                if childs[i].is_some() {
                    has_pruned = true;
                }
            }
        }

        if has_pruned {
            let mut old_update_cell = BuilderData::new();
            old_update_cell.set_type(old_cell.cell_type());
            for (i, child_opt) in childs.into_iter().enumerate() {
                let child = match child_opt {
                    None => {
                        let child = old_cell.reference(i)?;
                        Self::make_pruned_branch_cell(&child, merkle_depth)?
                    }
                    Some(child) => child,
                };
                old_update_cell.checked_append_reference(child.into_cell()?)?;
            }

            old_update_cell.append_bytestring(&SliceData::load_cell_ref(old_cell)?)?;
            Ok(Some(old_update_cell))
        } else {
            Ok(None)
        }
    }

    fn add_one_hash(mask: u8, depth: u8) -> Result<LevelMask> {
        if depth > 2 {
            fail!(BlockError::InvalidArg("depth".to_string()))
        } else if mask & (1 << depth) != 0 {
            fail!(BlockError::InvalidOperation(format!(
                "attempt to add hash with depth {} into mask {:03b}",
                depth, mask
            )))
        }
        Ok(LevelMask::with_mask(mask | (1 << depth)))
    }

    pub(crate) fn make_pruned_branch_cell(cell: &Cell, merkle_depth: u8) -> Result<BuilderData> {
        let mut result = BuilderData::new();
        let level_mask = Self::add_one_hash(cell.level_mask().mask(), merkle_depth)?;
        result.set_type(CellType::PrunedBranch);
        result.append_u8(u8::from(CellType::PrunedBranch))?;
        result.append_u8(level_mask.mask())?;
        for hash in cell.hashes() {
            result.append_raw(hash.as_slice(), hash.as_slice().len() * 8)?;
        }
        for depth in cell.depths() {
            result.append_u16(depth)?;
        }
        Ok(result)
    }

    pub(crate) fn make_pruned_branch_cell_by_hash(
        repr_hash: &UInt256,
        repr_depth: u16,
        merkle_depth: u8,
    ) -> Result<BuilderData> {
        let mut result = BuilderData::new();
        let level_mask = Self::add_one_hash(0, merkle_depth)?;
        result.set_type(CellType::PrunedBranch);
        result.append_u8(u8::from(CellType::PrunedBranch))?;
        result.append_u8(level_mask.mask())?;
        result.append_raw(repr_hash.as_slice(), repr_hash.as_slice().len() * 8)?;
        result.append_u16(repr_depth)?;
        Ok(result)
    }

    fn traverse_old_on_check(
        cell: &Cell,
        known_cells: &mut ahash::AHashSet<UInt256>,
        visited: &mut ahash::AHashSet<UInt256>,
        merkle_depth: u8,
        pruned_cells_count: &mut usize,
    ) -> Result<()> {
        if visited.insert(cell.repr_hash().clone()) {
            known_cells.insert(cell.hash(merkle_depth as usize).clone());
            if cell.cell_type() != CellType::PrunedBranch {
                let child_merkle_depth =
                    if cell.is_merkle() { merkle_depth + 1 } else { merkle_depth };
                let refs = cell.clone_references()?;
                for child in refs.iter() {
                    Self::traverse_old_on_check(
                        child,
                        known_cells,
                        visited,
                        child_merkle_depth,
                        pruned_cells_count,
                    )?;
                }
            } else {
                *pruned_cells_count += 1;
            }
        }
        Ok(())
    }

    // Checks all pruned branches from new tree are exist in old tree
    fn traverse_new_on_check(
        cell: &Cell,
        known_cells: &ahash::AHashSet<UInt256>,
        visited: &mut ahash::AHashSet<UInt256>,
        merkle_depth: u8,
        new_cells_count: &mut usize,
        pruned_cells_count: &mut usize,
    ) -> Result<()> {
        if visited.insert(cell.repr_hash().clone()) {
            if cell.cell_type() == CellType::PrunedBranch {
                *pruned_cells_count += 1;
                if cell.level() == merkle_depth + 1
                    && !known_cells.contains(cell.hash(merkle_depth as usize))
                {
                    fail!("old and new trees mismatch {:x}", cell.hash(merkle_depth as usize))
                }
            } else {
                *new_cells_count += 1;
                let child_merkle_depth =
                    if cell.is_merkle() { merkle_depth + 1 } else { merkle_depth };
                let refs = cell.clone_references()?;
                for child in refs.iter() {
                    Self::traverse_new_on_check(
                        child,
                        known_cells,
                        visited,
                        child_merkle_depth,
                        new_cells_count,
                        pruned_cells_count,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn collect_old_cells(
        cell: &Cell,
        known_cells_hashes: &ahash::AHashSet<UInt256>,
        known_cells: &mut ahash::AHashMap<UInt256, Cell>,
        visited: &mut ahash::AHashSet<UInt256>,
        merkle_depth: u8,
    ) -> Result<()> {
        if visited.insert(cell.repr_hash().clone()) {
            let hash = cell.hash(merkle_depth as usize).clone();
            if known_cells_hashes.contains(&hash) {
                known_cells.insert(hash, cell.clone());
                let child_merkle_depth =
                    if cell.is_merkle() { merkle_depth + 1 } else { merkle_depth };
                let refs = cell.clone_references()?;
                for child in refs.iter() {
                    Self::collect_old_cells(
                        child,
                        known_cells_hashes,
                        known_cells,
                        visited,
                        child_merkle_depth,
                    )?;
                }
            }
        }
        Ok(())
    }
}
