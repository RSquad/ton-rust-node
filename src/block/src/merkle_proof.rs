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
    accounts::Account,
    blocks::{Block, BlockInfo, BlockSeqNoAndShard},
    dictionary::hashmapaug::HashmapAugType,
    error::BlockError,
    fail,
    merkle_update::MerkleUpdate,
    messages::Message,
    shard::ShardStateUnsplit,
    transactions::Transaction,
    types::UInt256,
    BuilderData, Cell, CellType, Deserializable, GetRepresentationHash, IBitstring, Result,
    Serializable, SliceData, UsageTree,
};
use std::cmp::max;

#[cfg(test)]
#[path = "tests/test_merkle_proof.rs"]
mod tests;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MerkleProof {
    pub hash: UInt256,
    pub depth: u16,
    pub proof: Cell,
}

impl Deserializable for MerkleProof {
    fn read_from(&mut self, cell: &mut SliceData) -> Result<()> {
        if cell.pos() != 0 {
            fail!("Merkle proof have to fill full cell from its zeroth bit.")
        }
        if CellType::try_from(cell.get_next_byte()?)? != CellType::MerkleProof {
            fail!(BlockError::InvalidData("invalid Merkle proof root's cell type".to_string()))
        }
        self.hash.read_from(cell)?;
        self.depth = cell.get_next_u16()?;
        self.proof = cell.checked_drain_reference()?;
        if self.hash != *Cell::hash(&self.proof, 0) {
            fail!(BlockError::WrongMerkleProof(
                "Stored proof hash is not equal calculated one".to_string()
            ))
        }
        if self.depth != Cell::depth(&self.proof, 0) {
            fail!(BlockError::WrongMerkleProof(
                "Stored proof depth is not equal calculated one".to_string()
            ))
        }
        Ok(())
    }
}

impl Serializable for MerkleProof {
    fn write_to(&self, cell: &mut BuilderData) -> Result<()> {
        if !cell.is_empty() {
            fail!("Merkle proof have to fill full cell from its zeroth bit.")
        }
        cell.set_type(CellType::MerkleProof);
        cell.append_u8(u8::from(CellType::MerkleProof))?;
        self.hash.write_to(cell)?;
        cell.append_u16(self.depth)?;
        cell.checked_append_reference(self.proof.clone())?;
        Ok(())
    }
}

impl MerkleProof {
    /// Creating of a Merkle proof which includes cells whose hashes contain in `proof_for`.
    pub fn create(root: &Cell, is_include: impl Fn(&UInt256) -> bool) -> Result<Self> {
        if !is_include(root.repr_hash()) {
            fail!(BlockError::InvalidArg(
                "`bag` doesn't contain any cell to include into proof".to_string()
            ))
        }
        let mut done_cells = ahash::AHashMap::new();
        let proof =
            MerkleProof::create_raw(root, &is_include, &|_| false, 0, &mut None, &mut done_cells)?;

        Ok(MerkleProof { hash: root.repr_hash().clone(), depth: root.repr_depth(), proof })
    }

    /// Creating of a Merkle proof which includes cells whose hashes contain in `proof_for`.
    pub fn create_by_usage_tree(root: &Cell, usage_tree: &UsageTree) -> Result<Self> {
        MerkleProof::create(root, |h| usage_tree.contains(h))
    }

    pub fn create_with_subtrees(
        root: &Cell,
        is_include: impl Fn(&UInt256) -> bool,
        is_include_subtree: impl Fn(&UInt256) -> bool,
    ) -> Result<Self> {
        let root_hash = root.repr_hash().clone();
        if !is_include(&root_hash) && !is_include_subtree(&root_hash) {
            fail!(BlockError::InvalidArg(
                "`bag` doesn't contain any cell to include into proof".to_string()
            ))
        }
        let mut done_cells = ahash::AHashMap::new();
        let proof = MerkleProof::create_raw(
            root,
            &is_include,
            &is_include_subtree,
            0,
            &mut None,
            &mut done_cells,
        )?;

        Ok(MerkleProof { hash: root_hash, depth: root.repr_depth(), proof })
    }

    pub fn create_raw(
        cell: &Cell,
        is_include: &impl Fn(&UInt256) -> bool,
        is_include_subtree: &impl Fn(&UInt256) -> bool,
        merkle_depth: u8,
        pruned_branches: &mut Option<ahash::AHashSet<UInt256>>,
        done_cells: &mut ahash::AHashMap<UInt256, Cell>, // map with pruned cells
    ) -> Result<Cell> {
        let child_merkle_depth = if cell.is_merkle() { merkle_depth + 1 } else { merkle_depth };

        let data = smallvec::SmallVec::from_slice(cell.data());
        let mut proof_cell = BuilderData::with_raw(data, cell.bit_length())?;
        proof_cell.set_type(cell.cell_type());
        let n = cell.references_count();
        for i in 0..n {
            let child_repr_hash = cell.reference_repr_hash(i)?;

            let proof_child = if let Some(c) = done_cells.get(&child_repr_hash) {
                c.clone()
            } else if is_include_subtree(&child_repr_hash) {
                cell.reference(i)?
            } else if is_include(&child_repr_hash) {
                Self::create_raw(
                    &cell.reference(i)?,
                    is_include,
                    is_include_subtree,
                    child_merkle_depth,
                    pruned_branches,
                    done_cells,
                )?
            } else {
                let pbc = if cell.level() == 0 && cell.cell_type() == CellType::Ordinary {
                    // If current cell is ordinary and has zero level - child cell
                    // is ordinary and zero level too. So it is need only hash and depth
                    // to make pruned branch cell (no need to load child cell)
                    MerkleUpdate::make_pruned_branch_cell_by_hash(
                        &child_repr_hash,
                        cell.reference_repr_depth(i)?,
                        child_merkle_depth,
                    )?
                } else {
                    MerkleUpdate::make_pruned_branch_cell(&cell.reference(i)?, child_merkle_depth)?
                };
                if let Some(pruned_branches) = pruned_branches.as_mut() {
                    pruned_branches.insert(child_repr_hash);
                }
                pbc.into_cell()?
            };
            proof_cell.checked_append_reference(proof_child)?;
        }

        let proof_cell = proof_cell.into_cell()?;
        done_cells.insert(cell.repr_hash().clone(), proof_cell.clone());

        Ok(proof_cell)
    }

    pub fn virtualize<T: Deserializable>(&self) -> Result<T> {
        let virt_root = self.proof.clone().virtualize(1);
        T::construct_from_cell(virt_root)
    }
}

// checks if proof contains correct block info
pub fn check_block_info_proof(
    block: &Block,
    proof_hash: &UInt256,
    block_hash: &UInt256,
) -> Result<BlockInfo> {
    if proof_hash != block_hash {
        fail!(BlockError::WrongMerkleProof("Proof hash is not equal given block hash".to_string()))
    }
    block.read_info()
}

/// checks if transaction with given id is exist in block.
/// Proof must contain transaction's root cell and block info
pub fn check_transaction_proof(
    proof: &MerkleProof,
    tr: &Transaction,
    block_id: &UInt256,
) -> Result<()> {
    let block: Block = proof.virtualize().map_err(|err| {
        BlockError::WrongMerkleProof(format!("Error extracting block from proof: {}", err))
    })?;

    // check if block id in transaction is corresponds to block in proof
    let block_info = check_block_info_proof(&block, &proof.hash, block_id)?;

    // check if acc is belonged the block's shard
    if !block_info.shard().contains_account(tr.account_id())? {
        fail!(BlockError::WrongMerkleProof(
            "Account address in transaction belongs other shardchain".to_string()
        ))
    }

    // check if transaction is potencially belonged the block by logical time
    if tr.logical_time() < block_info.start_lt() || tr.logical_time() > block_info.end_lt() {
        fail!(BlockError::WrongMerkleProof(
            "Transaction's logical time doesn't belong to \
                 block's logical time interval"
                .to_string()
        ))
    }

    // read account block from block and check it

    let block_extra = block.read_extra().map_err(|err| {
        BlockError::WrongMerkleProof(format!("Error extracting block extra from proof: {}", err))
    })?;

    let account_blocks = block_extra.read_account_blocks().map_err(|err| {
        BlockError::WrongMerkleProof(format!("Error extracting account blocks from proof: {}", err))
    })?;

    let account_block = account_blocks
        .get_serialized(tr.account_id().clone())?
        .ok_or_else(|| BlockError::WrongMerkleProof("No account block in proof".to_string()))?;

    // find transaction
    let tr_parent_slice_opt =
        account_block.transactions().get_as_slice(&tr.logical_time()).map_err(|err| {
            BlockError::WrongMerkleProof(format!(
                "Error extracting transaction from dictionary in proof: {}",
                err
            ))
        })?;
    if let Some(mut tr_parent_slice) = tr_parent_slice_opt {
        if let Ok(tr_slice) = tr_parent_slice.checked_drain_reference() {
            // check hash
            if *tr_slice.repr_hash() != tr.hash()? {
                fail!(BlockError::WrongMerkleProof("Wrong transaction's hash in proof".to_string()))
            }
        }
    } else {
        fail!(BlockError::WrongMerkleProof("No transaction in proof".to_string()))
    }
    Ok(())
}

fn check_transaction_id(given_id: Option<UInt256>, tr_cell: Option<Cell>) -> Result<()> {
    let existing_id = tr_cell.map(|c| c.repr_hash().clone());
    match (given_id, existing_id) {
        (None, Some(_)) => {
            fail!(BlockError::WrongMerkleProof(
                "Invalid transaction id: None is passed, \
                     but the transaction exists in a block"
                    .to_string()
            ))
        }
        (Some(_), None) => {
            fail!(BlockError::WrongMerkleProof(
                "Invalid transaction id: it is passed, \
                     but the transaction doesn't exists in a block"
                    .to_string()
            ))
        }
        (None, None) => Ok(()),
        (Some(id1), Some(id2)) => {
            if id1 != id2 {
                fail!(BlockError::WrongMerkleProof("Invalid transaction id".to_string()))
            }
            Ok(())
        }
    }
}

/// checks if message with given id is exist in block.
/// Proof must contain message's root cell and block info
pub fn check_message_proof(
    proof: &MerkleProof,
    msg: &Message,
    block_id: &UInt256,
    tr_id: Option<UInt256>,
) -> Result<()> {
    let block: Block = proof.virtualize().map_err(|err| {
        BlockError::WrongMerkleProof(format!("Error extracting block from proof: {}", err))
    })?;

    // check if block id in message is corresponds to block in proof
    check_block_info_proof(&block, &proof.hash, block_id)?;

    // read message from block and check it

    let block_extra = block.read_extra().map_err(|err| {
        BlockError::WrongMerkleProof(format!("Error extracting block extra from proof: {}", err))
    })?;

    let msg_hash = msg.hash()?;
    // attempt to read in msg descr, if fail - read out one
    if let Ok(in_msg_descr) = block_extra.read_in_msg_descr() {
        if let Ok(Some(in_msg)) = in_msg_descr.get(&msg_hash) {
            check_transaction_id(tr_id, in_msg.transaction_cell())?;
            if let Ok(msg_cell) = in_msg.message_cell() {
                if *msg_cell.repr_hash() != msg_hash {
                    fail!(BlockError::WrongMerkleProof(format!(
                        "Wrong message's hash in proof {:x} but {:x}",
                        msg_cell.repr_hash(),
                        msg_hash
                    )))
                } else {
                    return Ok(());
                }
            } else {
                fail!(BlockError::WrongMerkleProof(
                    "Error extracting message from in message".to_string()
                ))
            }
        }
    }

    let out_msg_descr = block_extra.read_out_msg_descr().map_err(|err| {
        BlockError::WrongMerkleProof(format!("Error extracting out msg descr from proof: {}", err))
    })?;
    if let Ok(Some(out_msg)) = out_msg_descr.get(&msg_hash) {
        if let Ok(real_msg_hash) = out_msg.read_message_hash() {
            check_transaction_id(tr_id, out_msg.transaction_cell())?;
            if real_msg_hash != msg_hash {
                fail!(BlockError::WrongMerkleProof("Wrong message's hash in proof".to_string()))
            } else {
                Ok(())
            }
        } else {
            fail!(BlockError::WrongMerkleProof(
                "Error extracting message from out message".to_string()
            ))
        }
    } else {
        fail!(BlockError::WrongMerkleProof("No message in proof".to_string()))
    }
}

/// checks if account with given address is exist in shard state.
/// Proof must contain account's root cell
/// Returns info about the block corresponds to shard state the account belongs to.
pub fn check_account_proof(proof: &MerkleProof, acc: &Account) -> Result<BlockSeqNoAndShard> {
    if acc.is_none() {
        fail!(BlockError::InvalidData("Account can't be none".to_string()))
    }

    let ss: ShardStateUnsplit = proof.virtualize()?;

    let accounts = ss.read_accounts().map_err(|err| {
        BlockError::WrongMerkleProof(format!("Error extracting accounts dict from proof: {}", err))
    })?;

    let shard_acc = accounts.get_serialized(acc.get_id().unwrap().clone());
    if let Ok(Some(shard_acc)) = shard_acc {
        let acc_root = shard_acc.account_cell();
        let acc_hash = Cell::hash(&acc_root, (max(acc_root.level(), 1) - 1) as usize);
        if acc.hash()? != *acc_hash {
            fail!(BlockError::WrongMerkleProof("Wrong account's hash in proof".to_string()))
        } else {
            Ok(BlockSeqNoAndShard {
                seq_no: ss.seq_no(),
                vert_seq_no: ss.vert_seq_no(),
                shard_id: ss.shard().clone(),
            })
        }
    } else {
        fail!(BlockError::WrongMerkleProof("No account in proof".to_string()))
    }
}
