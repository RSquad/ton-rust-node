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
    profiling::InstanceCounter, receiver::ReceiverImpl, serialize_tl_boxed_object, ton, utils,
    utils::public_key_hash_to_int256, BlockHash, BlockHeight, BlockPayloadPtr, BlockSignature,
    CatchainFactory, PublicKeyHash, SessionId,
};
use std::{
    cell::RefCell,
    cmp,
    collections::HashMap,
    fmt,
    rc::{Rc, Weak},
};
use ton_api::IntoBoxed;
use ton_block::{error, fail, Result, UInt256};

/*
    Constants
*/

const BLOCK_SENDING_THROTTLING_DELAY: std::time::Duration = std::time::Duration::from_millis(500); //time delay for duplicate block resend

/// State of the received block
#[derive(PartialEq, Copy, Clone, Debug)]
pub(crate) enum ReceivedBlockState {
    /// Block is not initialized
    Null,

    /// Block is a part of fork
    Ill,

    /// Block is initialized
    Initialized,

    /// Block is delivered
    Delivered,
}

/*
    Implementation details for ReceivedBlock
    - is used as a temporary storage during the block receiving from the catchain
*/

/// Pointer to internal ReceivedBlock implementation
pub(crate) type ReceivedBlockPtr = Rc<RefCell<ReceivedBlock>>;

pub(crate) struct ReceivedBlock {
    self_cell: Weak<RefCell<ReceivedBlock>>, //back reference to itself to be used in recursive calls
    state: ReceivedBlockState,               //current block state
    height: BlockHeight,                     //height of the block
    incarnation: SessionId,                  //session ID for this block
    block_id_hash: BlockHash,                //hash of the block's ID
    data_payload_hash: BlockHash,            //hash of data
    source_id: usize, //receiver source which has generated & signed this block
    fork_id: usize,   //fork ID for this block inside current node
    prev: Option<ReceivedBlockPtr>, //previous block in a fork chain
    next: Option<Weak<RefCell<ReceivedBlock>>>, //next block in a fork chain
    signature: BlockSignature, //signature of the block
    payload: BlockPayloadPtr, //block's payload (for validator session)
    block_deps: Vec<ReceivedBlockPtr>, //dependencies which have been used for this block creation
    rev_deps: Vec<Weak<RefCell<ReceivedBlock>>>, //reverse dependecies (dependent blocks)
    forks_dep_heights: Vec<BlockHeight>, //heights of each fork which is used in prev & dependency blocks for this block
    pending_deps: usize, //number of pending dependencies for this block to be fully received
    in_db: bool,         //flag which showes that this block has been written to DB
    is_custom: bool, //flag which showes that this block should be processed by validator session
    last_sending_times: HashMap<PublicKeyHash, std::time::SystemTime>, //last sending time for the source
    serialized_block_with_payload: Option<BlockPayloadPtr>, //serialized block with payload
    _instance_counter: InstanceCounter,                     //received blocks instance counter
    get_pending_deps_call_id: u64, //unique ID for calling get_pending_deps (to cut off duplications during the blocks graph traverse)
}

/*
    Implementation of ReceiverBlock
*/

impl ReceivedBlock {
    /*
        General purpose methods & accessors
    */

    /// Back reference to itself for recursive methods
    fn get_self(&self) -> ReceivedBlockPtr {
        self.self_cell.upgrade().unwrap()
    }

    pub(crate) fn get_state(&self) -> ReceivedBlockState {
        self.state
    }

    pub(crate) fn is_initialized(&self) -> bool {
        matches!(self.state, ReceivedBlockState::Initialized | ReceivedBlockState::Delivered)
    }

    pub(crate) fn in_db(&self) -> bool {
        self.in_db
    }

    pub(crate) fn is_delivered(&self) -> bool {
        self.state == ReceivedBlockState::Delivered
    }

    pub(crate) fn is_custom(&self) -> bool {
        self.is_custom
    }

    pub(crate) fn get_height(&self) -> BlockHeight {
        self.height
    }

    pub(crate) fn get_source_id(&self) -> usize {
        self.source_id
    }

    pub(crate) fn get_fork_id(&self) -> usize {
        self.fork_id
    }

    pub(crate) fn get_hash(&self) -> &BlockHash {
        &self.block_id_hash
    }

    pub(crate) fn get_payload(&self) -> &BlockPayloadPtr {
        &self.payload
    }

    /*
        Dependencies management
    */

    pub(crate) fn get_prev(&self) -> Option<ReceivedBlockPtr> {
        match &self.prev {
            None => None,
            Some(prev) => Some(Rc::clone(prev)),
        }
    }

    pub(crate) fn get_prev_hash(&self) -> Option<BlockHash> {
        self.get_prev().map(|prev| prev.borrow().get_hash().clone())
    }

    fn get_next(&self) -> Option<ReceivedBlockPtr> {
        match &self.next {
            None => None,
            Some(next) => next.upgrade(),
        }
    }

    pub(crate) fn get_forks_dep_heights(&self) -> &Vec<BlockHeight> {
        &self.forks_dep_heights
    }

    pub(crate) fn get_dep_hashes(&self) -> Vec<BlockHash> {
        let mut hashes = Vec::with_capacity(self.block_deps.len());

        for it in &self.block_deps {
            hashes.push(it.borrow().get_hash().clone());
        }

        hashes
    }

    pub(crate) fn has_rev_deps(&self) -> bool {
        !self.rev_deps.is_empty()
    }

    pub(crate) fn get_pending_deps(
        &mut self,
        call_id: u64,
        max_deps_count: usize,
        dep_hashes: &mut Vec<BlockHash>,
    ) {
        if self.get_pending_deps_call_id == call_id {
            return; //ignore this subgraph because it has been already processed during the current sync
        }

        self.get_pending_deps_call_id = call_id;

        if self.get_height() == 0
            || self.get_state() == ReceivedBlockState::Ill
            || self.is_delivered()
            || dep_hashes.len() == max_deps_count
        {
            return;
        }

        if !self.is_initialized() {
            dep_hashes.push(self.get_hash().clone());
            return;
        }

        if let Some(prev) = self.get_prev() {
            prev.borrow_mut().get_pending_deps(call_id, max_deps_count, dep_hashes);
        }

        for it in &self.block_deps {
            it.borrow_mut().get_pending_deps(call_id, max_deps_count, dep_hashes);
        }
    }

    fn update_forks_dependency_heights(&mut self, block: &ReceivedBlock) {
        let len = self.forks_dep_heights.len();
        for (i, actual) in block.forks_dep_heights.iter().enumerate() {
            if len <= i {
                self.forks_dep_heights.push(*actual)
            } else {
                self.forks_dep_heights[i] = self.forks_dep_heights[i].max(*actual)
            }
        }
    }

    fn add_rev_dep(&mut self, block: &ReceivedBlock) {
        self.rev_deps.push(block.self_cell.clone());
    }

    /*
        Block fork management
    */

    fn set_ill(&mut self, receiver: &mut ReceiverImpl) {
        if self.state == ReceivedBlockState::Ill {
            return;
        }

        log::warn!("Ill block detected: {}", self.get_hash().to_hex_string());

        receiver.get_source(self.source_id).borrow_mut().mark_as_blamed(receiver);

        self.state = ReceivedBlockState::Ill;

        for block in &self.rev_deps {
            block.upgrade().unwrap().borrow_mut().set_ill(receiver);
        }
    }

    /*
        Delivery management
    */

    pub(crate) fn process(&mut self, receiver: &mut ReceiverImpl) {
        match self.get_state() {
            ReceivedBlockState::Null | ReceivedBlockState::Delivered | ReceivedBlockState::Ill => {
                return
            }
            _ => (),
        }

        assert!(self.get_state() == ReceivedBlockState::Initialized);
        assert!(self.pending_deps == 0);
        assert!(self.in_db);

        self.initialize_fork(receiver);
        self.pre_deliver(receiver);
        self.deliver(receiver);
    }

    pub(crate) fn written(&mut self, receiver: &mut ReceiverImpl) {
        if self.in_db {
            return;
        }

        self.in_db = true;

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...block {:?} has been written to DB, pending deps is {}",
                &self.get_hash(),
                self.pending_deps
            );
        }

        if self.pending_deps == 0 {
            self.schedule(receiver);
        }
    }

    fn schedule(&mut self, receiver: &mut ReceiverImpl) {
        if log::log_enabled!(log::Level::Trace) {
            log::trace!("...schedule block {:?} for delivering", &self.get_hash());
        }

        receiver.run_block(self.get_self());
    }

    fn initialize_fork(&mut self, receiver: &mut ReceiverImpl) {
        assert!(self.state == ReceivedBlockState::Initialized);
        assert!(self.fork_id == 0);

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...initialize fork for block {:?} at height {} from source {}",
                &self.get_hash(),
                self.height,
                self.source_id
            );
        }

        let source = receiver.get_source(self.source_id);

        if self.height == 1 {
            self.fork_id = source.borrow_mut().add_fork(receiver);
        } else {
            assert!(self.prev.is_some());

            let prev = self.prev.as_ref().unwrap();

            if prev.borrow().get_next().is_some() {
                self.fork_id = source.borrow_mut().add_fork(receiver);
            } else {
                prev.borrow_mut().next = Some(self.self_cell.clone());
                self.fork_id = prev.borrow().get_fork_id();
            }
        }

        if self.forks_dep_heights.len() < self.fork_id + 1 {
            self.forks_dep_heights.resize(self.fork_id + 1, 0);
        }

        assert!(self.forks_dep_heights[self.fork_id] < self.height);

        self.forks_dep_heights[self.fork_id] = self.height;
    }

    fn pre_deliver(&mut self, receiver: &mut ReceiverImpl) {
        if self.state == ReceivedBlockState::Ill {
            return;
        }

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...check block {:?} dependencies before delivering (pre-deliver)",
                &self.get_hash()
            );
        }

        assert!(self.state == ReceivedBlockState::Initialized);
        assert!(self.pending_deps == 0);
        assert!(self.in_db);

        let source = receiver.get_source(self.source_id);

        if let Some(prev) = self.prev.clone() {
            let prev_borrowed = prev.borrow_mut();
            let prev_hash = prev_borrowed.get_hash().clone();
            let prev_forks_dep_heights = &prev_borrowed.forks_dep_heights;
            let block_deps_for_iteration = self.block_deps.clone();

            for dep_it in &block_deps_for_iteration {
                let dep = dep_it.borrow();
                let dep_source = receiver.get_source(dep.get_source_id());
                let dep_source = dep_source.borrow_mut();
                let dep_source_fork_ids = &dep_source.get_forks();

                if dep.get_fork_id() < prev_forks_dep_heights.len()
                    && dep.get_height() <= prev_forks_dep_heights[dep.get_fork_id()]
                {
                    log::warn!(
                        "Block {:?} has direct dependency {:?} with fork_id={} and height={} \
                        from source #{} and prev block {:?} has newer indirect dependency \
                        with the same fork and height={}",
                        dep.get_hash(),
                        dep.get_hash(),
                        dep.get_fork_id(),
                        dep.get_height(),
                        dep_source.get_id(),
                        prev_hash,
                        prev_forks_dep_heights[dep.get_fork_id()]
                    );

                    self.set_ill(receiver);

                    return;
                }

                if !dep_source.is_blamed() {
                    continue;
                }

                for &dep_source_fork_id in *dep_source_fork_ids {
                    if dep_source_fork_id == dep.get_fork_id()
                        || prev_forks_dep_heights.len() <= dep_source_fork_id
                        || prev_forks_dep_heights[dep_source_fork_id] == 0
                    {
                        continue;
                    }

                    log::warn!(
                        "Block {:?} has direct dependency {:?} with fork_id={} and height={} \
                        from source #{} and prev block {:?} has indirect dependency \
                        to another fork fork_id={} and height={} of the same source",
                        dep.get_hash(),
                        dep.get_hash(),
                        dep.get_fork_id(),
                        dep.get_height(),
                        dep_source.get_id(),
                        prev.borrow().get_hash(),
                        dep_source_fork_id,
                        prev_forks_dep_heights[dep_source_fork_id]
                    );

                    source.borrow_mut().blame(self.fork_id, self.height, receiver);

                    self.set_ill(receiver);

                    return;
                }

                let dep_source_blamed_heights = dep_source.get_blamed_heights();
                let iterations_count =
                    cmp::min(dep_source_blamed_heights.len(), prev_forks_dep_heights.len());

                for fork_id in 0..iterations_count {
                    if dep_source_blamed_heights[fork_id] == 0
                        || prev_forks_dep_heights[fork_id] < dep_source_blamed_heights[fork_id]
                    {
                        continue;
                    }

                    log::warn!(
                        "Block {:?} has direct dependency {:?} with fork_id={} and height={} \
                        from source #{} and prev block {:?} has indirect dependency \
                        to fork fork_id={} and height={} which is known to blame this source",
                        dep.get_hash(),
                        dep.get_hash(),
                        dep.get_fork_id(),
                        dep.get_height(),
                        dep_source.get_id(),
                        prev.borrow().get_hash(),
                        fork_id,
                        dep_source_blamed_heights[fork_id]
                    );

                    source.borrow_mut().blame(self.fork_id, self.height, receiver);

                    self.set_ill(receiver);

                    return;
                }
            }
        }

        use ton_api::ton::catchain::block::inner::Data;

        match ton_api::Deserializer::new(&mut self.payload.data().as_slice()).read_boxed::<Data>() {
            Ok(message) => match message {
                Data::Catchain_Block_Data_Fork(message) => {
                    let left = message.left.only();
                    let right = message.right.only();

                    if let Err(err) = receiver.validate_block_dependency(&left) {
                        log::warn!("Incorrect fork blame: left is ivalid: {:?}", err);
                        self.set_ill(receiver);
                        return;
                    }
                    if let Err(err) = receiver.validate_block_dependency(&right) {
                        log::warn!("Incorrect fork blame: right is ivalid: {:?}", err);
                        self.set_ill(receiver);
                        return;
                    }

                    if left.height != right.height
                        || left.src != right.src
                        || left.data_hash == right.data_hash
                    {
                        log::warn!(
                            "Incorrect fork blame, not a fork: {}/{}, {}/{}, {:?}/{:?}",
                            left.height,
                            right.height,
                            left.src,
                            right.src,
                            left.data_hash,
                            right.data_hash
                        );
                        self.set_ill(receiver);
                        return;
                    }

                    let source = receiver.get_source(left.src as usize);
                    let fork_proof =
                        ton::BlockDataFork { left: left.into_boxed(), right: right.into_boxed() };

                    source.borrow_mut().set_fork_proof(fork_proof);
                    source.borrow_mut().mark_as_blamed(receiver);
                }
                Data::Catchain_Block_Data_Nop | Data::Catchain_Block_Data_BadBlock(_) => { /*do nothing*/
                }
                #[allow(unreachable_patterns)]
                //only for forward compatibility with new types in C++ node
                _ => self.is_custom = true,
            },
            Err(_err) => self.is_custom = true,
        }
    }

    fn deliver(&mut self, receiver: &mut ReceiverImpl) {
        if self.state == ReceivedBlockState::Ill {
            return;
        }

        if log::log_enabled!(log::Level::Trace) {
            log::trace!("...prepare block {:?} for delivering", &self.get_hash());
        }

        assert!(self.state == ReceivedBlockState::Initialized);
        assert!(self.pending_deps == 0);
        assert!(self.in_db);

        receiver.deliver_block(self);

        self.state = ReceivedBlockState::Delivered;

        if log::log_enabled!(log::Level::Trace) {
            log::trace!("...block {:?} has been delivered", self.get_hash());
        }

        for rev_dep_it in &self.rev_deps {
            if let Some(rev_dep) = rev_dep_it.upgrade() {
                rev_dep.borrow_mut().dependency_delivered(self, receiver);
            }
        }

        self.rev_deps.clear();

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...notify source #{} about new block {:?} which is ready to be delivered",
                self.get_source_id(),
                self.get_hash()
            );
        }

        receiver.get_source(self.source_id).borrow_mut().block_delivered(self.height);
    }

    fn dependency_delivered(&mut self, block: &ReceivedBlock, receiver: &mut ReceiverImpl) {
        if self.state == ReceivedBlockState::Ill {
            return;
        }

        assert!(block.get_state() != ReceivedBlockState::Ill);

        self.update_forks_dependency_heights(block);

        self.pending_deps -= 1;

        if self.pending_deps == 0 && self.in_db {
            self.schedule(receiver);
        }
    }

    /*
        Block pre validation
    */

    pub(crate) fn pre_validate_block_dependency(
        receiver: &ReceiverImpl,
        block: &ton::BlockDep,
    ) -> Result<()> {
        if block.height < 0 {
            fail!("Invalid height {}", block.height);
        }

        if block.height > 0 {
            if block.src < 0 || block.src as usize >= receiver.get_sources_count() {
                fail!("Invalid source {}", block.src);
            }
        } else {
            if block.src < 0 || block.src as usize != receiver.get_sources_count() {
                fail!("Invalid source (first block) {}", block.src);
            }

            if (&block.data_hash != receiver.get_incarnation()) || (!block.signature.is_empty()) {
                fail!("Invalid first block");
            }
        }

        Ok(())
    }

    pub(crate) fn pre_validate_block(
        receiver: &ReceiverImpl,
        block: &ton::Block,
        payload: &BlockPayloadPtr,
    ) -> Result<()> {
        if &block.incarnation != receiver.get_incarnation() {
            fail!("Invalid session ID");
        }

        if block.height <= 0 {
            fail!("Invalid height {}", block.height);
        }

        if block.src < 0 || block.src as usize >= receiver.get_sources_count() {
            fail!("Invalid source {}", block.src);
        }

        if block.data.prev.src < 0 {
            fail!("Invalid prev block source {}", block.data.prev.src);
        }

        if block.data.deps.len() > receiver.get_options().max_deps as usize {
            fail!("Too many deps");
        }

        let prev_src = block.data.prev.src as usize;

        if block.height > 1 {
            if prev_src != block.src as usize {
                fail!("Invalid prev block source {}", block.data.prev.src);
            }
        } else if prev_src != receiver.get_sources_count() {
            fail!("Invalid prev(first) block source {}", block.data.prev.src);
        }

        if block.data.prev.height + 1 != block.height {
            fail!("Invalid prev block height {} (our {})", block.data.prev.height, block.height);
        }

        use std::collections::HashSet;

        let mut used: HashSet<i32> = HashSet::new();

        used.insert(block.src);

        for dep in &block.data.deps {
            if used.contains(&dep.src) {
                fail!("Two deps from the same source");
            }

            used.insert(dep.src);
        }

        (receiver.validate_block_dependency(&block.data.prev))?;

        for dep in &block.data.deps {
            (receiver.validate_block_dependency(dep))?;
        }

        if payload.data().is_empty() {
            fail!("Empty payload");
        }

        Ok(())
    }

    /*
        Throttling & serialization cache
    */

    pub(crate) fn mark_block_for_sending(&mut self, adnl_id: &PublicKeyHash) -> bool {
        //check if block is requested to resend earlier than throttling limit

        if let Some(last_send_time) = self.last_sending_times.get(adnl_id) {
            if let Ok(delay) = last_send_time.elapsed() {
                if delay < BLOCK_SENDING_THROTTLING_DELAY {
                    return false;
                }
            }
        }

        //update trhottling

        self.last_sending_times.insert(adnl_id.clone(), std::time::SystemTime::now());

        true
    }

    pub(crate) fn get_serialized_block_with_payload(&mut self) -> &BlockPayloadPtr {
        if self.serialized_block_with_payload.is_some() {
            return self.serialized_block_with_payload.as_ref().unwrap();
        }

        let block_update_event = ton::BlockUpdateEvent { block: self.export_tl() }.into_boxed();
        let mut serialized_message = serialize_tl_boxed_object!(&block_update_event);

        serialized_message.extend(self.payload.data().iter());

        let serialized_block_with_payload =
            CatchainFactory::create_block_payload(serialized_message);

        self.serialized_block_with_payload = Some(serialized_block_with_payload);

        self.serialized_block_with_payload.as_ref().unwrap()
    }

    /*
        TL export
    */

    pub(crate) fn export_tl(&self) -> ton::Block {
        assert!(self.is_initialized());
        assert!(self.height > 0);

        let mut deps = Vec::new();

        for dep in &self.block_deps {
            deps.push(dep.borrow().export_tl_dep());
        }

        let block_data =
            ton::BlockData { prev: self.get_prev().unwrap().borrow().export_tl_dep(), deps };

        ton::Block {
            incarnation: self.incarnation.clone(),
            src: self.source_id as i32,
            height: self.height,
            data: block_data,
            signature: self.signature.clone(),
        }
    }

    pub(crate) fn export_tl_dep(&self) -> ton::BlockDep {
        ton::BlockDep {
            src: self.source_id as i32,
            data_hash: self.data_payload_hash.clone(),
            height: self.height,
            signature: self.signature.clone(),
        }
    }

    /*
        Block initialization
    */

    pub(crate) fn initialize(
        &mut self,
        block: &ton::Block,
        payload: BlockPayloadPtr,
        receiver: &mut ReceiverImpl,
    ) -> Result<()> {
        if self.state != ReceivedBlockState::Null {
            return Ok(());
        }

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...initialize block {:?} with payload of {} bytes",
                self.block_id_hash,
                payload.data().len()
            );
        }

        assert!(!payload.data().is_empty());
        self.payload = payload;

        let prev = receiver.create_block(&block.data.prev);
        self.prev = Some(prev.clone());

        for dep in &block.data.deps {
            let dep_block = receiver.create_block(dep).clone();
            self.block_deps.push(dep_block);
        }

        self.signature = block.signature.clone();
        self.state = ReceivedBlockState::Initialized;

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...check prev block and dependencies for {:?} to be initialized",
                self.block_id_hash
            );
        }

        if prev.borrow().get_state() == ReceivedBlockState::Ill {
            let warning = error!(
                "...prev block {:?} for block {:?} is ill",
                self.block_id_hash,
                prev.borrow().get_hash()
            );

            log::warn!("{}", warning);
            self.set_ill(receiver);
            return Err(warning);
        }

        for dep in &self.block_deps {
            if dep.borrow().get_state() == ReceivedBlockState::Ill {
                let warning = error!(
                    "...dependency block {:?} for block {:?} is ill",
                    self.block_id_hash,
                    dep.borrow().get_hash()
                );

                log::warn!("{}", warning);
                self.set_ill(receiver);
                return Err(warning);
            }
        }

        if log::log_enabled!(log::Level::Trace) {
            log::trace!("...compute forks dependency heights for {:?}", self.block_id_hash);
        }

        let mut pending_deps: usize = 0;

        if !prev.borrow().is_delivered() {
            pending_deps += 1;
        } else {
            self.update_forks_dependency_heights(&*prev.borrow());
        }
        if !prev.borrow().is_delivered() {
            prev.borrow_mut().add_rev_dep(self);
        }

        let block_deps_for_iteration = self.block_deps.clone();

        for dep in &block_deps_for_iteration {
            if !dep.borrow().is_delivered() {
                pending_deps += 1;
            } else {
                self.update_forks_dependency_heights(&*dep.borrow());
            }
            if !dep.borrow().is_delivered() {
                dep.borrow_mut().add_rev_dep(self);
            }
        }

        self.pending_deps = pending_deps;

        if log::log_enabled!(log::Level::Trace) {
            log::trace!("...pending {} dependencies for {:?}", self.pending_deps, self.get_hash());
        }

        if self.pending_deps == 0 && self.in_db {
            self.schedule(receiver);
        }

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...notify source #{} about new received block {:?}",
                self.source_id,
                self.get_hash()
            );
        }

        receiver.get_source(self.source_id).borrow_mut().block_received(self.height);

        Ok(())
    }

    fn new(instance_counter: &InstanceCounter) -> Self {
        ReceivedBlock {
            self_cell: Weak::new(),
            state: ReceivedBlockState::Null,
            height: 0,
            incarnation: BlockHash::default(),
            data_payload_hash: UInt256::default(),
            block_id_hash: UInt256::default(),
            source_id: 0,
            fork_id: 0,
            prev: None,
            next: None,
            signature: BlockSignature::default(),
            payload: CatchainFactory::create_empty_block_payload(),
            block_deps: Vec::new(),
            rev_deps: Vec::new(),
            forks_dep_heights: Vec::new(),
            pending_deps: 0,
            in_db: false,
            is_custom: false,
            last_sending_times: HashMap::new(),
            serialized_block_with_payload: None,
            _instance_counter: instance_counter.clone(),
            get_pending_deps_call_id: 0,
        }
    }

    fn wrap(block: &mut Rc<RefCell<ReceivedBlock>>) -> Rc<RefCell<ReceivedBlock>> {
        block.borrow_mut().self_cell = Rc::downgrade(block);
        block.clone()
    }

    pub(crate) fn create_root(
        source_id: usize,
        incarnation: &SessionId,
        instance_counter: &InstanceCounter,
    ) -> ReceivedBlockPtr {
        let mut body: ReceivedBlock = ReceivedBlock::new(instance_counter);
        let block_id = utils::get_root_block_id(incarnation);

        body.source_id = source_id;
        body.data_payload_hash = incarnation.clone();
        body.state = ReceivedBlockState::Delivered;
        body.block_id_hash = utils::get_block_id_hash(&block_id);
        body.incarnation = incarnation.clone();

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...create root block: \
                hash={:?}, source_id={}, height={}, data_payload_hash={:?}, signature={:?}",
                body.block_id_hash,
                body.source_id,
                body.height,
                body.data_payload_hash,
                body.signature
            );
        }

        ReceivedBlock::wrap(&mut Rc::new(RefCell::new(body)))
    }

    pub(crate) fn create_with_payload(
        block: &ton::Block,
        payload: BlockPayloadPtr,
        receiver: &mut ReceiverImpl,
    ) -> Result<ReceivedBlockPtr> {
        let mut body: ReceivedBlock =
            ReceivedBlock::new(receiver.get_received_blocks_instance_counter());

        let block_id = utils::get_block_id(
            receiver.get_incarnation(),
            receiver.get_source_public_key_hash(block.src as usize),
            block,
            payload.data(),
            &receiver.get_options(),
        );

        body.data_payload_hash = block_id.data_hash().clone();
        body.signature = block.signature.clone();
        body.block_id_hash = utils::get_block_id_hash(&block_id);
        body.source_id = block.src as usize;
        body.height = block.height as BlockHeight;
        body.incarnation = receiver.get_incarnation().clone();

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...create new block with payload: \
                hash={:?}, source_id={}, height={}, data_payload_hash={:?}, signature={:?}",
                body.block_id_hash,
                body.source_id,
                body.height,
                body.data_payload_hash,
                body.signature
            )
        }

        assert!(
            body.height as u64
                <= crate::receiver::get_max_block_height(
                    &receiver.get_options(),
                    receiver.get_sources_count()
                )
        );

        let new_block = ReceivedBlock::wrap(&mut Rc::new(RefCell::new(body)));

        let source = receiver.get_source(block.src as usize);

        source.borrow_mut().process_new_block(new_block.clone(), receiver);

        new_block.borrow_mut().initialize(block, payload, receiver)?;

        Ok(new_block)
    }

    pub(crate) fn get_block_dep_hash(block: &ton::BlockDep, receiver: &ReceiverImpl) -> BlockHash {
        utils::get_block_id_hash(
            &::ton_api::ton::catchain::block::id::Id {
                incarnation: receiver.get_incarnation().clone(),
                src: public_key_hash_to_int256(
                    receiver.get_source_public_key_hash(block.src as usize),
                ),
                height: block.height,
                data_hash: block.data_hash.clone(),
            }
            .into_boxed(),
        )
    }

    pub(crate) fn create(block: &ton::BlockDep, receiver: &mut ReceiverImpl) -> ReceivedBlockPtr {
        let mut body: ReceivedBlock =
            ReceivedBlock::new(receiver.get_received_blocks_instance_counter());

        body.data_payload_hash = block.data_hash.clone();
        body.signature = block.signature.clone();
        body.block_id_hash = Self::get_block_dep_hash(block, receiver);
        body.source_id = block.src as usize;
        body.height = block.height as BlockHeight;
        body.incarnation = receiver.get_incarnation().clone();

        if log::log_enabled!(log::Level::Trace) {
            log::trace!(
                "...create new block dependency: \
                hash={:?}, source_id={}, height={}, data_hash={:?}, signature={:?}",
                body.block_id_hash,
                body.source_id,
                body.height,
                body.data_payload_hash,
                body.signature
            )
        }

        let new_block = ReceivedBlock::wrap(&mut Rc::new(RefCell::new(body)));

        let source = receiver.get_source(block.src as usize);

        source.borrow_mut().process_new_block(new_block.clone(), receiver);

        new_block
    }
}

/*
    Implementation for display ReceivedBlock trait
*/

impl fmt::Display for ReceivedBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "ReceivedBlock(hash={:?}, source_id={}, height={})",
            &self.get_hash(),
            self.source_id,
            self.height
        )
    }
}
