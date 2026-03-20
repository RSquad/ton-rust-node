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
    Block, BlockExtraId, BlockHash, BlockHeight, BlockPayloadPtr, BlockPtr, PublicKeyHash,
};
use std::{fmt, sync::Arc};

/*
    Implementation details for Block
*/

pub(crate) struct BlockImpl {
    source_id: usize, //receiver source which has generated & signed this block
    fork_id: usize,   //fork ID for this block inside current node
    source_public_key_hash: PublicKeyHash, //public key hash of the source
    height: BlockHeight, //height of the block
    hash: BlockHash,  //hash of the block
    prev: Option<BlockPtr>, //previous block in a fork chain
    payload: BlockPayloadPtr, //block's payload (for validator session)
    block_deps: Vec<BlockPtr>, //dependencies for this block
    forks_dep_heights: Vec<BlockHeight>, //heights of each fork which is used in prev & dependency blocks for this block
    extra_id: BlockExtraId,              //block extra data identifier
    creation_time: std::time::SystemTime, //block creation time
}

/*
    Implementation for public Block trait
*/

impl Block for BlockImpl {
    /*
        General purpose methods & accessors
    */

    fn get_creation_time(&self) -> std::time::SystemTime {
        self.creation_time
    }

    fn get_extra_id(&self) -> BlockExtraId {
        self.extra_id
    }

    fn get_payload(&self) -> &BlockPayloadPtr {
        &self.payload
    }

    fn get_source_id(&self) -> u32 {
        self.source_id as u32
    }

    fn get_fork_id(&self) -> usize {
        self.fork_id
    }

    fn get_source_public_key_hash(&self) -> &PublicKeyHash {
        &self.source_public_key_hash
    }

    fn get_hash(&self) -> &BlockHash {
        &self.hash
    }

    fn get_height(&self) -> BlockHeight {
        self.height
    }

    fn get_prev(&self) -> Option<BlockPtr> {
        self.prev.clone()
    }

    fn get_deps(&self) -> &Vec<BlockPtr> {
        &self.block_deps
    }

    fn get_forks_dep_heights(&self) -> &Vec<BlockHeight> {
        &self.forks_dep_heights
    }

    fn is_descendant_of(&self, block: &dyn Block) -> bool {
        let fork = block.get_fork_id();

        if fork >= self.forks_dep_heights.len() {
            return false;
        }

        block.get_height() <= self.forks_dep_heights[fork]
    }
}

impl fmt::Display for BlockImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Block(hash={:?}, source_id={}, height={})",
            &self.hash, self.source_id, self.height
        )
    }
}

impl fmt::Debug for BlockImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Block")
            .field("hash", &self.hash)
            .field("source_id", &self.source_id)
            .field("height", &self.height)
            .finish()
    }
}

impl BlockImpl {
    /*
        Block creation
    */

    pub(crate) fn create(
        source_id: usize,
        fork_id: usize,
        source_public_key_hash: PublicKeyHash,
        height: BlockHeight,
        hash: BlockHash,
        payload: BlockPayloadPtr,
        prev_block: Option<BlockPtr>,
        block_deps: Vec<BlockPtr>,
        forks_dep_heights: Vec<BlockHeight>,
        extra_id: BlockExtraId,
    ) -> BlockPtr {
        let body = BlockImpl {
            source_id,
            fork_id,
            source_public_key_hash,
            height,
            hash,
            prev: prev_block,
            payload,
            block_deps,
            forks_dep_heights,
            extra_id,
            creation_time: std::time::SystemTime::now(),
        };

        Arc::new(body)
    }
}
