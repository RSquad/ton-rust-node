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
    receiver::ReceiverImpl, serialize_tl_boxed_object, ton, utils, BlockHeight, BlockPayloadPtr,
    CatchainFactory, PublicKey, PublicKeyHash, ReceivedBlockPtr,
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};
use ton_api::IntoBoxed;

/// Source statistics
#[derive(Default, Debug)]
pub(crate) struct ReceiverSourceStatistics {
    /// Number of incoming received queries
    pub in_queries_count: usize,

    /// Number of outgoing sent queries
    pub out_queries_count: usize,

    /// Number of incoming received messages
    pub in_messages_count: usize,

    /// Number of outgoing sent messages
    pub out_messages_count: usize,

    /// Number of received broadcasts
    pub in_broadcasts_count: usize,
}

/*
    Implementation details for ReceiverSource
    - it contains validator's knowledge about other validator
*/

/// Pointer to internal ReceiverSource implementation
pub(crate) type ReceiverSourcePtr = Rc<RefCell<ReceiverSource>>;

pub(crate) struct ReceiverSource {
    id: usize,                                       //source identifier
    adnl_id: PublicKeyHash,                          //ADNL identifier of the source
    public_key: PublicKey,                           //public key of the source
    public_key_hash: PublicKeyHash,                  //public key hash of the source
    blamed: bool,                                    //is this source blamed
    blocks: BTreeMap<BlockHeight, ReceivedBlockPtr>, //map from height to block for this source - our knowledge about the source chain
    delivered_height: BlockHeight, //how many blocks have been delivered for this source
    received_height: BlockHeight,  //how many blocks have been received for this source
    fork_proof: Option<ton::BlockDataFork>, //fork proof
    fork_proof_serialized: Option<BlockPayloadPtr>, //fork proof serialized
    fork_ids: Vec<usize>,          //fork identifiers for this source
    blamed_heights: Vec<BlockHeight>, //blamed heights for each fork id
    statistics: ReceiverSourceStatistics, //source statistics
}

/*
    Implementation of ReceiverSource
*/

impl ReceiverSource {
    /*
        Accessors
    */

    pub(crate) fn get_id(&self) -> usize {
        self.id
    }

    pub(crate) fn get_public_key_hash(&self) -> &PublicKeyHash {
        &self.public_key_hash
    }

    pub(crate) fn get_public_key(&self) -> &PublicKey {
        &self.public_key
    }

    pub(crate) fn get_adnl_id(&self) -> &PublicKeyHash {
        &self.adnl_id
    }

    pub(crate) fn is_blamed(&self) -> bool {
        self.blamed
    }

    pub(crate) fn get_statistics(&self) -> &ReceiverSourceStatistics {
        &self.statistics
    }

    pub(crate) fn get_mut_statistics(&mut self) -> &mut ReceiverSourceStatistics {
        &mut self.statistics
    }

    /*
        Blocks management
    */

    pub(crate) fn get_block(&self, height: BlockHeight) -> Option<ReceivedBlockPtr> {
        match self.blocks.get(&height) {
            None => None,
            Some(t) => Some(t.clone()),
        }
    }

    pub(crate) fn process_new_block(
        &mut self,
        block_cell: ReceivedBlockPtr,
        receiver: &mut ReceiverImpl,
    ) {
        if self.is_fork_found() {
            return;
        }

        let block = block_cell.borrow();

        assert!(block.get_source_id() == self.id);

        if let Some(existing_block) = self.get_block(block.get_height()) {
            assert!(block.get_hash() != existing_block.borrow().get_hash());

            log::warn!(
                "fork found on height {} for source #{}: blocks {} and {}",
                block.get_height(),
                self.id,
                block.get_hash().to_hex_string(),
                existing_block.borrow().get_hash().to_hex_string()
            );

            if !self.is_fork_found() {
                let fork = ton::BlockDataFork {
                    left: block.export_tl_dep().into_boxed(),
                    right: existing_block.borrow().export_tl_dep().into_boxed(),
                };

                self.set_fork_proof(fork);

                receiver.add_fork_proof(self.id, self.fork_proof_serialized.as_ref().unwrap());
            }

            self.mark_as_blamed(receiver);
            return;
        }

        self.blocks.insert(block.get_height(), block_cell.clone());
    }

    /*
        Forks management
    */

    pub(crate) fn get_forks_count(&self) -> usize {
        self.fork_ids.len()
    }

    pub(crate) fn add_fork(&mut self, receiver: &mut ReceiverImpl) -> usize {
        if !self.fork_ids.is_empty() {
            self.mark_as_blamed(receiver);
        }

        let fork_id = receiver.add_fork();

        assert!(fork_id > 0);

        self.fork_ids.push(fork_id);

        log::trace!("...adding new fork {} of source {}", fork_id, self.id);

        if self.fork_ids.len() > 1 {
            assert!(self.is_blamed());
        }

        fork_id
    }

    pub(crate) fn blame(&mut self, fork: usize, height: BlockHeight, receiver: &mut ReceiverImpl) {
        self.mark_as_blamed(receiver);

        //associate blamed height with a fork id
        //we don't check blamed_heights.len() > 0 because it's a dead code in original TON implementation

        if self.blamed_heights.len() <= fork {
            self.blamed_heights.resize(fork + 1, 0);
        }

        if self.blamed_heights[fork] == 0 || self.blamed_heights[fork] > height {
            log::info!("Source {} has been blamed at fork {} and height {}", self.id, fork, height);
            self.blamed_heights[fork] = height;
        }
    }

    pub(crate) fn mark_as_blamed(&mut self, receiver: &mut ReceiverImpl) {
        if !self.blamed {
            log::debug!("Blaming source {}", self.id);

            self.blocks.clear();
            self.delivered_height = 0;

            receiver.blame(self.id);
        }

        self.blamed = true;
    }

    pub(crate) fn get_forks(&self) -> &[usize] {
        &self.fork_ids
    }

    pub(crate) fn get_blamed_heights(&self) -> &Vec<BlockHeight> {
        &self.blamed_heights
    }

    pub(crate) fn is_fork_found(&self) -> bool {
        self.fork_proof.is_some()
    }

    pub(crate) fn set_fork_proof(&mut self, fork_proof: ton::BlockDataFork) {
        if self.is_fork_found() {
            return;
        }

        self.fork_proof = Some(fork_proof.clone());
        self.fork_proof_serialized = Some(CatchainFactory::create_block_payload(
            serialize_tl_boxed_object!(&fork_proof.into_boxed()),
        ));

        log::error!(
            "Fork has been found for source {} hash={:?}",
            self.get_id(),
            utils::get_hash(self.fork_proof_serialized.as_ref().unwrap().data())
        );
    }

    pub(crate) fn get_fork_proof(&self) -> &Option<ton::BlockDataFork> {
        &self.fork_proof
    }

    /*
        Receivement & Delivery management
    */

    pub(crate) fn has_unreceived(&self) -> bool {
        if self.is_blamed() {
            return true;
        }

        if self.blocks.is_empty() {
            return false;
        }

        let (_, last_received_block) = self.blocks.iter().next_back().unwrap();
        let last_received_block_height = last_received_block.borrow().get_height();

        assert!(last_received_block_height >= self.received_height);

        last_received_block_height > self.received_height
    }

    pub(crate) fn has_undelivered(&self) -> bool {
        self.delivered_height < self.received_height
    }

    pub(crate) fn get_received_height(&self) -> BlockHeight {
        self.received_height
    }

    pub(crate) fn get_delivered_height(&self) -> BlockHeight {
        self.delivered_height
    }

    pub(crate) fn block_received(&mut self, height: BlockHeight) {
        if self.is_blamed() {
            return;
        }

        if self.received_height + 1 == height {
            self.received_height = height;
        }

        loop {
            let block_result = self.get_block(self.received_height + 1);

            if block_result.is_none() {
                return;
            }

            let block = block_result.unwrap();

            if !block.borrow().is_initialized() {
                return;
            }

            self.received_height += 1;
        }
    }

    pub(crate) fn block_delivered(&mut self, height: BlockHeight) {
        if self.is_blamed() {
            return;
        }

        if self.delivered_height + 1 == height {
            self.delivered_height = height;
        }

        loop {
            let block_result = self.get_block(self.delivered_height + 1);

            if block_result.is_none() {
                return;
            }

            let block = block_result.unwrap();

            if !block.borrow().is_delivered() {
                return;
            }

            self.delivered_height += 1;
        }
    }

    /*
        Creation
    */

    fn new(id: usize, public_key: PublicKey, adnl_id: &PublicKeyHash) -> Self {
        let public_key_hash = utils::get_public_key_hash(&public_key);

        log::trace!(
            "...creating source #{} with public_key_hash={}, adnl_id={}",
            id,
            public_key_hash,
            adnl_id
        );

        Self {
            id,
            adnl_id: adnl_id.clone(),
            public_key,
            public_key_hash,
            blamed: false,
            delivered_height: 0,
            received_height: 0,
            blocks: BTreeMap::new(),
            fork_proof: None,
            fork_proof_serialized: None,
            fork_ids: Vec::new(),
            blamed_heights: Vec::new(),
            statistics: ReceiverSourceStatistics::default(),
        }
    }

    pub(crate) fn create(
        id: usize,
        public_key: PublicKey,
        adnl_id: &PublicKeyHash,
    ) -> ReceiverSourcePtr {
        Rc::new(RefCell::new(ReceiverSource::new(id, public_key, adnl_id)))
    }
}

impl Drop for ReceiverSource {
    fn drop(&mut self) {
        log::trace!(
            "...dropping source #{} with public_key_hash={}, adnl_id={}",
            self.id,
            self.public_key_hash,
            self.adnl_id
        );

        //avoid stack overflow for long block chains (single-node mode with fast block generation)

        while let Some((key, _item)) = self.blocks.iter().next_back() {
            let key = key.clone();
            //remove block with max height (because it has smallest dropping chain)
            self.blocks.remove(&key);
        }
    }
}
