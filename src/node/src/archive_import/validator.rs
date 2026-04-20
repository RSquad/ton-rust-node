/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
use crate::{block_proof::BlockProofStuff, shard_state::ShardStateStuff};
use std::sync::Arc;
use ton_block::{BlockIdExt, BlockInfo, Result};

pub struct ValidatorState {
    zerostate: Arc<ShardStateStuff>,
    current_key_block_proof: Option<BlockProofStuff>,
    hardforks: Vec<BlockIdExt>,
}

impl ValidatorState {
    pub fn new(zerostate: Arc<ShardStateStuff>, hardforks: Vec<BlockIdExt>) -> Self {
        Self { zerostate, current_key_block_proof: None, hardforks }
    }

    pub(crate) fn is_hardfork(&self, block_id: &BlockIdExt) -> bool {
        self.hardforks.iter().any(|hf| hf == block_id)
    }

    pub(crate) fn zerostate(&self) -> &Arc<ShardStateStuff> {
        &self.zerostate
    }

    pub(crate) fn current_key_block_proof(&self) -> Option<&BlockProofStuff> {
        self.current_key_block_proof.as_ref()
    }

    pub fn set_key_block_proof(&mut self, proof: BlockProofStuff) {
        self.current_key_block_proof = Some(proof);
    }

    pub fn validate_mc_proof(&mut self, proof: &BlockProofStuff) -> Result<BlockInfo> {
        let (virt_block, _virt_root) = proof.virtualize_block()?;
        let info = virt_block.read_info()?;

        let prev_key_block_seqno = info.prev_key_block_seqno();

        if prev_key_block_seqno == 0 {
            proof.check_with_master_state(&self.zerostate)?;
        } else {
            let prev_key_proof = self.current_key_block_proof.as_ref().ok_or_else(|| {
                ton_block::error!(
                    "No key block proof available for validation of block {} \
                     (prev_key_block_seqno = {})",
                    proof.id(),
                    prev_key_block_seqno
                )
            })?;
            proof.check_with_prev_key_block_proof(prev_key_proof)?;
        }

        if info.key_block() {
            self.current_key_block_proof = Some(proof.clone());
        }

        Ok(info)
    }

    pub fn extract_mc_info(&mut self, proof: &BlockProofStuff) -> Result<BlockInfo> {
        let (virt_block, _virt_root) = proof.virtualize_block()?;
        let info = virt_block.read_info()?;

        if info.key_block() {
            self.current_key_block_proof = Some(proof.clone());
        }

        Ok(info)
    }

    pub fn validate_shard_proof_link(&self, proof: &BlockProofStuff) -> Result<()> {
        proof.check_proof_link()
    }
}
