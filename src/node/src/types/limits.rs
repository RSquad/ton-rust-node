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
use std::{cmp::max, collections::HashSet, sync::Arc};
use ton_block::{BlockLimits, Cell, ParamLimitIndex, Result, UInt256, UsageTree};

pub struct BlockLimitStatus {
    accounts: u32,
    gas_used: u32,
    limits: Arc<BlockLimits>,
    lt_current: u64,
    lt_start: u64,
    in_msgs: u32,        // counts all incoming messages added to the block
    out_msgs: u32,       // counts all outgoing messages added to the block
    extra_out_msgs: u32, // counts all outgoing messages including those over the limit it could be decreased later
    removed_split_msgs: u32,
    dispatch_queue_ops: u32, // counts the number of operations on the dictionary
    out_msg_queue_ops: u32,
    stats: CellStorageStats,
    transactions: u32,
}

impl BlockLimitStatus {
    /// Constructor
    pub fn with_limits(limits: Arc<BlockLimits>) -> Self {
        Self {
            accounts: 0,
            gas_used: 0,
            limits,
            lt_current: 0,
            lt_start: u64::MAX,
            in_msgs: 0,
            out_msgs: 0,
            extra_out_msgs: 0,
            removed_split_msgs: 0,
            dispatch_queue_ops: 0,
            out_msg_queue_ops: 0,
            stats: CellStorageStats::default(),
            transactions: 0,
        }
    }

    /// Add gas usage
    pub fn add_gas_used(&mut self, gas: u32) {
        self.gas_used += gas;
    }

    pub fn gas_used(&self) -> u32 {
        self.gas_used
    }

    /// Add transactions
    pub fn add_transaction(&mut self, account: bool) {
        self.transactions += 1;
        if account {
            self.accounts += 1;
        }
    }

    pub fn add_cell(&mut self, env_cell: &Cell) -> Result<()> {
        self.stats.add_cell(env_cell)
    }

    /// Classify the status
    pub fn classify(&self, pruned_count: usize) -> ParamLimitIndex {
        self.limits
            .bytes()
            .classify(self.estimate_block_size(None, pruned_count))
            .max(self.limits.gas().classify(self.gas_used))
            .max(self.limits.lt_delta().classify(self.lt_delta()))
    }

    /// Register operation with output message queue in the block
    pub fn register_out_msg_queue_op(
        &mut self,
        root: Option<&Cell>,
        usage_tree: &UsageTree,
        force: bool,
    ) -> Result<()> {
        self.out_msg_queue_ops += 1;
        if force || ((self.out_msg_queue_ops & 63) == 0) {
            if let Some(root) = root {
                self.stats.add_proof(root, usage_tree)?;
            }
        }
        Ok(())
    }

    pub fn register_dispatch_queue_op(
        &mut self,
        root: Option<&Cell>,
        usage_tree: &UsageTree,
        force: bool,
    ) -> Result<()> {
        self.dispatch_queue_ops += 1;
        if force || (self.dispatch_queue_ops & 63) == 0 {
            if let Some(root) = root {
                self.stats.add_proof(root, usage_tree)?;
            }
        }
        Ok(())
    }

    pub fn register_in_msg_op(&mut self, msg_cell: &Cell, msg_dict: &Cell) -> Result<()> {
        self.in_msgs += 1;
        self.stats.add_cell(msg_cell)?;
        if (self.in_msgs & 63) == 0 {
            self.stats.add_cell(msg_dict)?;
        }
        Ok(())
    }

    pub fn register_out_msg_op(&mut self, msg_cell: &Cell, msg_dict: &Cell) -> Result<()> {
        self.out_msgs += 1;
        self.extra_out_msgs += 1;
        self.stats.add_cell(msg_cell)?;
        if (self.out_msgs & 63) == 0 {
            self.stats.add_cell(msg_dict)?;
        }
        Ok(())
    }

    pub fn remove_extra_out_msg_op(&mut self) {
        self.extra_out_msgs = self.extra_out_msgs.saturating_sub(1);
    }

    pub fn register_remove_split_msg(&mut self) {
        self.removed_split_msgs += 1;
    }

    /// Update logical time
    pub fn update_lt(&mut self, lt: u64) {
        self.lt_current = max(self.lt_current, lt);
        if self.lt_start > self.lt_current {
            self.lt_start = lt;
        }
    }

    pub fn lt(&self) -> u64 {
        self.lt_current
    }

    pub fn estimate_block_size(
        &self,
        extra: Option<&CellStorageStats>,
        pruned_count: usize,
    ) -> u32 {
        let mut ret = 2000 + pruned_count as u32 * 20 + self.removed_split_msgs * 128;
        ret += self.extra_out_msgs * 300 + self.accounts * 200 + self.transactions * 200;
        let mut bits = self.stats.cells_stats.bits + self.stats.proof_stats.bits;
        let mut cels = self.stats.cells_stats.cells + self.stats.proof_stats.cells;
        let mut ints = self.stats.cells_stats.internal_refs + self.stats.proof_stats.internal_refs;
        let mut exts = self.stats.cells_stats.external_refs + self.stats.proof_stats.external_refs;
        if let Some(extra) = extra {
            bits += extra.cells_stats.bits;
            cels += extra.cells_stats.cells;
            ints += extra.cells_stats.internal_refs;
            exts += extra.cells_stats.external_refs;
            ret += 200;
        }
        ret += (bits >> 3) + cels * 12 + ints * 3 + exts * 40;
        // log::debug!("ESTIMATE: size: {}, cells: {:?}, proofs: {:?}, removed: {}, split msgs: {}",
        //     ret, self.stats.cells_stats, self.stats.proof_stats, self.out_msg_queue_ops, self.removed_split_msgs);
        ret
    }

    pub fn fits(&self, level: ParamLimitIndex, pruned_count: usize) -> bool {
        let bytes = self.estimate_block_size(None, pruned_count);
        self.limits.fits(level, bytes, self.gas_used, self.lt_delta())
    }

    fn lt_delta(&self) -> u32 {
        self.lt_current.saturating_sub(self.lt_start) as u32
    }
    /*
        pub fn dump_block_size(&self) {
            dbg!(self.stats.cells_stats.bits, self.stats.proof_stats.bits);
            dbg!(self.stats.cells_stats.cells, self.stats.proof_stats.cells);
            dbg!(self.stats.cells_stats.internal_refs, self.stats.proof_stats.internal_refs);
            dbg!(self.stats.cells_stats.external_refs, self.stats.proof_stats.external_refs);
            dbg!(self.accounts, self.transactions);
            dbg!(self.estimate_block_size(None));
        }
    */
}

#[derive(Default, Debug)]
struct Stats {
    bits: u32,
    cells: u32,
    internal_refs: u32,
    external_refs: u32,
}

#[derive(Default)]
pub struct CellStorageStats {
    cells_seen: HashSet<UInt256>,
    cells_stats: Stats,
    proof_seen: HashSet<UInt256>,
    proof_stats: Stats,
}

impl CellStorageStats {
    pub fn add_cell(&mut self, cell: &Cell) -> Result<()> {
        self.traverse(cell, true, false, None)
    }

    pub fn add_proof(&mut self, cell: &Cell, usage_tree: &UsageTree) -> Result<()> {
        self.traverse(cell, false, true, Some(usage_tree))
    }

    fn traverse(
        &mut self,
        cell: &Cell,
        mut cells_stat: bool,
        mut proof_stat: bool,
        usage_tree: Option<&UsageTree>,
    ) -> Result<()> {
        /*
          if (cell.is_null()) {
            // FIXME: save error flag?
            return;
          }
        */
        if cells_stat {
            self.cells_stats.internal_refs += 1;
            /* if (parent_ && parent_->seen_.count(cell->get_hash()) != 0) || */
            if !self.cells_seen.insert(cell.repr_hash()) {
                // This cell and its children had been already seen
                cells_stat = false
            } else {
                self.cells_stats.cells += 1
            }
        }
        /*
          if (need_stat) {
            stat_.internal_refs++;
            if ((parent_ && parent_->seen_.count(cell->get_hash()) != 0) || !seen_.insert(cell->get_hash()).second) {
              need_stat = false;
            } else {
              stat_.cells++;
            }
          }
        */

        if proof_stat {
            if Some(true) == usage_tree.map(|usage_tree| usage_tree.contains(&cell.repr_hash())) {
                self.proof_stats.external_refs += 1;
                proof_stat = false;
            }
            /* auto tree_node = cell->get_tree_node();
            if (!tree_node.empty() && tree_node.is_from_tree(usage_tree_)) {
                proof_stat_.external_refs++;
                need_proof_stat = false;
            } else { */
            else {
                self.proof_stats.internal_refs += 1;
                // if (parent_ && parent_->proof_seen_.count(cell->get_hash()) != 0) ||
                if !self.proof_seen.insert(cell.repr_hash()) {
                    // This cell and its children had been already seen
                    proof_stat = false
                } else {
                    self.proof_stats.cells += 1
                }
            }
        }
        /*
          if (need_proof_stat) {
            auto tree_node = cell->get_tree_node();
            if (!tree_node.empty() && tree_node.is_from_tree(usage_tree_)) {
              proof_stat_.external_refs++;
              need_proof_stat = false;
            } else {
              proof_stat_.internal_refs++;
              if ((parent_ && parent_->proof_seen_.count(cell->get_hash()) != 0) ||
                  !proof_seen_.insert(cell->get_hash()).second) {
                need_proof_stat = false;
              } else {
                proof_stat_.cells++;
              }
            }
          }
        */

        if !cells_stat && !proof_stat {
            return Ok(());
        }
        /*
          if (!need_proof_stat && !need_stat) {
            return;
          }
        */

        let bits = cell.bit_length() as u32;
        if cells_stat {
            self.cells_stats.bits += bits
        }
        if proof_stat {
            self.proof_stats.bits += bits
        }
        /*
          vm::CellSlice cs{vm::NoVm{}, std::move(cell)};
          if (need_stat) {
            stat_.bits += cs.size();
          }
          if (need_proof_stat) {
            proof_stat_.bits += cs.size();
          }
        */
        for i in 0..cell.references_count() {
            self.traverse(&cell.reference(i)?, cells_stat, proof_stat, usage_tree)?
        }
        /*
          while (cs.size_refs()) {
            dfs(cs.fetch_ref(), need_stat, need_proof_stat);
          }
        */
        Ok(())
    }
}
