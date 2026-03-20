/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */
/*
    Internal tests with access to private crate symbols
*/

use super::*;
use catchain::serialize_tl_boxed_object;
use rand::Rng;
use std::{
    fmt,
    time::{Duration, SystemTime},
};
use ton_api::IntoBoxed;
use ton_block::{fail, KeyId, UInt256};

/*
    Constants
*/

const PERSISTENT_CACHE_SIZE: usize = 10000;
const TEMP_CACHE_SIZE: usize = 10000;

/*
    Implementation details for SessionDescription
*/

type CacheEntryPtr = Option<CacheEntry>;

struct SessionDescriptionImpl {
    options: SessionOptions,                          //validator session options
    hashes: Vec<PublicKeyHash>,                       //hashes
    persistent_objects_cache: Vec<CacheEntryPtr>,     //cache of persistent objects
    temp_objects_cache: Vec<CacheEntryPtr>,           //cache of temporary objects objects
    rng: rand::rngs::ThreadRng,                       //random generator
    metrics_receiver: catchain::utils::MetricsHandle, //receiver for profiling metrics
    sent_blocks_instance_counter: CachedInstanceCounter, //instance counter for sent blocks
    block_candidate_signatures_instance_counter: CachedInstanceCounter, //instance counter for block candidate signatures
    block_candidates_instance_counter: CachedInstanceCounter, //instance counter for block candidates
    vote_candidates_instance_counter: CachedInstanceCounter,  //instance counter for vote candidates
    round_attempts_instance_counter: CachedInstanceCounter,   //instance counter for round attempts
    rounds_instance_counter: CachedInstanceCounter,           //instance counter for rounds
    old_rounds_instance_counter: CachedInstanceCounter,       //instance counter for old rounds
    session_states_instance_counter: CachedInstanceCounter,   //instance counter for session states
    integer_vectors_instance_counter: CachedInstanceCounter,  //instance counter for integer vectors
    bool_vectors_instance_counter: CachedInstanceCounter,     //instance counter for bool vectors
    block_candidate_vectors_instance_counter: CachedInstanceCounter, //instance counter for block candidate vectors
    block_candidate_signature_vectors_instance_counter: CachedInstanceCounter, //instance counter for block candidate signatures vectors
    vote_candidate_vectors_instance_counter: CachedInstanceCounter, //instance counter for vote candidate vectors
    round_attempt_vectors_instance_counter: CachedInstanceCounter, //instance counter for round attempt vectors
    old_round_vectors_instance_counter: CachedInstanceCounter, //instance counter for old round vectors
}

/*
    Implementation for public SessionDescription trait
*/

impl SessionDescription for SessionDescriptionImpl {
    /*
        General purpose methods & accessors
    */

    fn opts(&self) -> &SessionOptions {
        &self.options
    }

    fn is_accelerated_consensus_enabled(&self) -> bool {
        self.options.accelerated_consensus_enabled
    }

    fn get_accelerated_consensus_initial_collator_index(&self) -> u32 {
        0
    }

    fn get_cache(&mut self) -> &mut dyn SessionCache {
        self
    }

    /*
        Validators management
    */

    fn get_source_public_key_hash(&self, src_idx: u32) -> &PublicKeyHash {
        &self.hashes[src_idx as usize]
    }

    fn get_source_public_key(&self, _src_idx: u32) -> &PublicKey {
        unreachable!();
    }

    fn get_source_adnl_id(&self, _src_idx: u32) -> &PublicKeyHash {
        unreachable!();
    }

    fn get_source_index(&self, public_key_hash: &PublicKeyHash) -> Result<u32> {
        Ok(public_key_hash.data()[0] as u32)
    }

    fn get_node_weight(&self, _src_idx: u32) -> ValidatorWeight {
        1
    }

    fn get_total_nodes(&self) -> u32 {
        self.hashes.len() as u32
    }

    fn get_self_idx(&self) -> u32 {
        unreachable!();
    }

    fn export_nodes(&self) -> Vec<PublicKeyHash> {
        unreachable!();
    }

    fn export_full_nodes(&self) -> Vec<PublicKey> {
        unreachable!();
    }

    fn export_catchain_nodes(&self) -> Vec<CatchainNode> {
        unreachable!();
    }

    /*
        Weights & priorities
    */

    fn get_cutoff_weight(&self) -> ValidatorWeight {
        (2 * self.hashes.len() / 3 + 1) as u64
    }

    fn get_reverse_cutoff_weight(&self) -> ValidatorWeight {
        (self.hashes.len() / 3 + 1) as u64
    }

    fn get_total_weight(&self) -> ValidatorWeight {
        self.hashes.len() as ValidatorWeight
    }

    fn get_normal_node_priority(&self, src_idx: u32, round: u32) -> i32 {
        let round = round % self.get_total_nodes();
        let src_idx = if src_idx < round { src_idx + self.get_total_nodes() } else { src_idx };

        if src_idx - round < self.options.round_candidates {
            return (src_idx - round) as i32;
        }

        -1
    }

    fn get_max_priority(&self) -> u32 {
        self.options.round_candidates - 1
    }

    fn get_vote_for_author(&self, attempt: u32) -> u32 {
        attempt % self.get_total_nodes()
    }

    /*
        Time management
    */

    fn set_time(&mut self, _time: std::time::SystemTime) {
        unreachable!();
    }

    fn get_time(&self) -> SystemTime {
        SystemTime::now()
    }

    fn is_in_future(&self, time: SystemTime) -> bool {
        time > self.get_time()
    }

    fn is_in_past(&self, time: SystemTime) -> bool {
        time < self.get_time()
    }

    fn get_unixtime(&self, ts: u64) -> u32 {
        (ts >> 32) as u32
    }

    fn get_attempt_sequence_number(&self, ts: u64) -> u32 {
        let round_attempt_duration_in_secs: u32 =
            self.options.round_attempt_duration.as_secs() as u32;
        self.get_unixtime(ts) / round_attempt_duration_in_secs
    }

    fn get_ts(&self) -> u64 {
        let now = self.get_time();
        let time_elapsed = match now.duration_since(std::time::UNIX_EPOCH) {
            Ok(elapsed) => elapsed.as_secs_f64(),
            Err(_err) => {
                log::error!("SessionDescription::get_ts: can't get system time");
                panic!("SessionDescription::get_ts");
            }
        };

        const TS_INTEGER_PART_MULTIPLIER: u64 = 1u64 << 32;

        let int_part = time_elapsed as u32;
        let frac_part =
            ((TS_INTEGER_PART_MULTIPLIER as f64) * (time_elapsed - (int_part as f64))) as u64;

        assert!(frac_part < TS_INTEGER_PART_MULTIPLIER);

        ((int_part as u64) << 32) + frac_part
    }

    fn get_delay(&self, mut _priority: u32) -> std::time::Duration {
        std::time::Duration::from_millis(0)
    }

    fn get_empty_block_delay(&self) -> std::time::Duration {
        std::time::Duration::from_millis(0)
    }

    fn get_attempt_start_at(&self, attempt: u32) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + attempt * self.options.round_attempt_duration
    }

    /*
        Signatures
    */

    fn candidate_id(
        &self,
        src_idx: u32,
        root_hash: &BlockHash,
        file_hash: &BlockHash,
        collated_data_file_hash: &BlockHash,
    ) -> BlockId {
        let candidate_id = ::ton_api::ton::validator_session::candidateid::CandidateId {
            src: self.get_source_public_key_hash(src_idx).data().into(),
            root_hash: root_hash.clone(),
            file_hash: file_hash.clone(),
            collated_data_file_hash: collated_data_file_hash.clone(),
        }
        .into_boxed();
        let serialized_candidate_id = serialize_tl_boxed_object!(&candidate_id);

        catchain::utils::get_hash(&serialized_candidate_id)
    }

    fn check_signature(
        &self,
        _root_hash: &BlockHash,
        _file_hash: &BlockHash,
        _src_idx: u32,
        signature: &BlockSignature,
    ) -> Result<()> {
        if signature.is_empty() {
            fail!("wrong size");
        }

        if signature[0] == 126 {
            Ok(())
        } else {
            fail!("invalid")
        }
    }

    fn check_approve_signature(
        &self,
        _root_hash: &BlockHash,
        _file_hash: &BlockHash,
        _src_idx: u32,
        signature: &BlockSignature,
    ) -> Result<()> {
        if signature.is_empty() {
            fail!("wrong size");
        }

        if signature[0] == 127 {
            Ok(())
        } else {
            fail!("invalid")
        }
    }

    /*
        Random
    */

    fn generate_random_usize(&mut self) -> usize {
        self.rng.gen()
    }

    /*
        Metrics
    */

    fn get_metrics_receiver(&self) -> &catchain::utils::MetricsHandle {
        &self.metrics_receiver
    }

    fn get_sent_blocks_instance_counter(&self) -> &CachedInstanceCounter {
        &self.sent_blocks_instance_counter
    }

    fn get_block_candidate_signatures_instance_counter(&self) -> &CachedInstanceCounter {
        &self.block_candidate_signatures_instance_counter
    }

    fn get_block_candidates_instance_counter(&self) -> &CachedInstanceCounter {
        &self.block_candidates_instance_counter
    }

    fn get_vote_candidates_instance_counter(&self) -> &CachedInstanceCounter {
        &self.vote_candidates_instance_counter
    }

    fn get_round_attempts_instance_counter(&self) -> &CachedInstanceCounter {
        &self.round_attempts_instance_counter
    }

    fn get_rounds_instance_counter(&self) -> &CachedInstanceCounter {
        &self.rounds_instance_counter
    }

    fn get_old_rounds_instance_counter(&self) -> &CachedInstanceCounter {
        &self.old_rounds_instance_counter
    }

    fn get_session_states_instance_counter(&self) -> &CachedInstanceCounter {
        &self.session_states_instance_counter
    }

    fn get_integer_vectors_instance_counter(&self) -> &CachedInstanceCounter {
        &self.integer_vectors_instance_counter
    }

    fn get_bool_vectors_instance_counter(&self) -> &CachedInstanceCounter {
        &self.bool_vectors_instance_counter
    }

    fn get_block_candidate_vectors_instance_counter(&self) -> &CachedInstanceCounter {
        &self.block_candidate_vectors_instance_counter
    }

    fn get_block_candidate_signature_vectors_instance_counter(&self) -> &CachedInstanceCounter {
        &self.block_candidate_signature_vectors_instance_counter
    }

    fn get_vote_candidate_vectors_instance_counter(&self) -> &CachedInstanceCounter {
        &self.vote_candidate_vectors_instance_counter
    }

    fn get_round_attempt_vectors_instance_counter(&self) -> &CachedInstanceCounter {
        &self.round_attempt_vectors_instance_counter
    }

    fn get_old_round_vectors_instance_counter(&self) -> &CachedInstanceCounter {
        &self.old_round_vectors_instance_counter
    }
}

/*
    Implementation for public Cache trait
*/

impl SessionCache for SessionDescriptionImpl {
    fn get_cache_entry_by_hash(&self, hash: HashType, allow_temp: bool) -> Option<&CacheEntry> {
        let mut cache_index = hash as usize % PERSISTENT_CACHE_SIZE;

        if let Some(ref entry) = self.persistent_objects_cache[cache_index] {
            return Some(entry);
        }

        if !allow_temp {
            return None;
        }

        cache_index = hash as usize % TEMP_CACHE_SIZE;

        match &self.temp_objects_cache[cache_index] {
            Some(entry) => Some(entry),
            _ => None,
        }
    }

    fn add_cache_entry(&mut self, hash: HashType, cache_entry: CacheEntry, pool: SessionPool) {
        let pool = match pool {
            SessionPool::Persistent => &mut self.persistent_objects_cache,
            SessionPool::Temp => &mut self.temp_objects_cache,
        };
        let cache_index = hash as usize % pool.len();

        pool[cache_index] = Some(cache_entry);
    }

    fn clear_temp_memory(&mut self) {
        let temp_pool_size = self.temp_objects_cache.len();

        self.temp_objects_cache.clear();
        self.temp_objects_cache.resize(temp_pool_size, None);
    }

    fn increment_reuse_counter(&mut self, _pool: SessionPool) {}
}

/*
    Implementation for public Display & Debug
*/

impl fmt::Display for SessionDescriptionImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

impl fmt::Debug for SessionDescriptionImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SessionDescription").field("options", &self.options).finish()
    }
}

/*
    Implementation internals of SessionDescriptionImpl
*/

impl SessionDescriptionImpl {
    pub(crate) fn new(options: &SessionOptions, total_nodes: u32) -> Self {
        let metrics_receiver = catchain::utils::MetricsHandle::new(Some(Duration::from_secs(30)));

        let mut hashes = Vec::new();

        for i in 0..total_nodes {
            let mut hash: [u8; 32] = [0; 32];

            hash[0] = i as u8;

            let hash = KeyId::from_data(hash);
            // let hash = PublicKeyHash

            hashes.push(hash);
        }

        Self {
            options: *options,
            persistent_objects_cache: vec![None; PERSISTENT_CACHE_SIZE],
            temp_objects_cache: vec![None; TEMP_CACHE_SIZE],
            rng: rand::thread_rng(),
            hashes,
            sent_blocks_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "sent_blocks",
            ),
            block_candidate_signatures_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "block_candidates_signatures",
            ),
            block_candidates_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "block_candidates",
            ),
            vote_candidates_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "vote_candidates",
            ),
            round_attempts_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "round_attempts",
            ),
            rounds_instance_counter: CachedInstanceCounter::new(&metrics_receiver, "rounds"),
            old_rounds_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "old_rounds",
            ),
            session_states_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "session_states",
            ),
            integer_vectors_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "integer_vectors",
            ),
            bool_vectors_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "bool_vectors",
            ),
            block_candidate_vectors_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "block_candidate_vectors",
            ),
            block_candidate_signature_vectors_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "block_candidate_signature_vectors",
            ),
            vote_candidate_vectors_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "vote_candidate_vectors",
            ),
            round_attempt_vectors_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "round_attempt_vectors",
            ),
            old_round_vectors_instance_counter: CachedInstanceCounter::new(
                &metrics_receiver,
                "old_round_vectors",
            ),
            metrics_receiver,
        }
    }
}

fn check_empty_actions(
    description: &mut dyn SessionDescription,
    state: &SessionStatePtr,
    attempt_id: u32,
) {
    let total_nodes = description.get_total_nodes();

    for i in 0..total_nodes {
        let action = state.create_action(description, i, attempt_id);

        assert!(action.is_some());

        let action = action.unwrap();

        assert!(
            matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
            "Action should be Empty instead of {:?}",
            action
        );
    }
}

pub fn test_state_hashes_part1(options: &SessionOptions, total_nodes: u32) {
    let mut description = SessionDescriptionImpl::new(options, total_nodes);
    let now = std::time::SystemTime::now();

    {
        //check primitives

        let x1: u32 = 1;
        let x2: bool = true;
        let x3: Option<u32> = None;

        assert!(x1.get_hash() == 932806723);
        assert!(x2.get_hash() == 831612713);
        assert!(x3.get_hash() == 0);
    }

    {
        //check empty state hash

        let state = SessionFactory::create_state(&mut description);

        assert!(state.get_hash() == 321933076);
    }

    {
        //check bool vector hash

        let v: Vec<bool> = vec![false; 3];
        let v = SessionFactory::create_bool_vector(&mut description, v);
        let v = v.change(&mut description, 0, true);

        assert!(v.get_hash() == 2502091135);
    }

    {
        //check vec of u32

        let v: Vec<u32> = vec![0; 3];
        let v = SessionFactory::create_vector_wrapper(&mut description, v);
        let v = v.change(&mut description, 0, 1);
        let v = v.change(&mut description, 1, 1);

        assert!(v.get_hash() == 2424132713);
    }

    let zero_hash = BlockHash::default();
    let c1 = description.candidate_id(0, &zero_hash, &zero_hash, &zero_hash);
    let c2 = description.candidate_id(1, &zero_hash, &zero_hash, &zero_hash);

    assert!(c1 != c2);

    let zero_hash: UInt256 = zero_hash;
    let mut state = SessionFactory::create_state(&mut description);
    let mut attempt_id = 1000000000;

    check_empty_actions(&mut description, &state, attempt_id);

    {
        //check block submission

        let message = ton::message::SubmittedBlock {
            round: 0,
            root_hash: zero_hash.clone(),
            file_hash: zero_hash.clone(),
            collated_data_file_hash: zero_hash.clone(),
        }
        .into_boxed();

        state = state.apply_action(&mut description, 1, attempt_id, &message, now, now);
        state = state.move_to_persistent(description.get_cache());

        assert!(state.get_hash() == 3718863710);
    }

    check_empty_actions(&mut description, &state, attempt_id);

    {
        //check approving & signing

        for i in 0..total_nodes {
            let block = state.choose_block_to_sign(&description, i);

            assert!(block.is_none());

            let blocks_to_approve = state.choose_blocks_to_approve(&description, i);

            assert!(blocks_to_approve.len() == 2);
            assert!(blocks_to_approve[0].is_some());
            assert!(blocks_to_approve[0].get_id() == &c2);
            assert!(blocks_to_approve[1].is_none());
            assert!(blocks_to_approve[1].get_id() == &*SKIP_ROUND_CANDIDATE_BLOCKID);
        }
    }

    {
        //check approving with signature #1

        for i in 0..2 * total_nodes / 3 {
            let signature = vec![127; 1];
            let action = ton::message::ApprovedBlock { round: 0, candidate: c2.clone(), signature }
                .into_boxed();

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_hash() == 3873054883);
    }

    check_empty_actions(&mut description, &state, attempt_id);

    {
        //check approving with signature #2

        for i in 2 * total_nodes / 3..total_nodes {
            let signature = vec![127; 1];
            let action = ton::message::ApprovedBlock { round: 0, candidate: c2.clone(), signature }
                .into_boxed();

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_hash() == 1013875306);
    }

    {
        //check approving & signing

        for i in 0..total_nodes {
            let block = state.choose_block_to_sign(&description, i);

            assert!(block.is_none());

            let blocks_to_approve = state.choose_blocks_to_approve(&description, i);

            assert!(blocks_to_approve.len() == 1);
            assert!(blocks_to_approve[0].is_none());
            assert!(blocks_to_approve[0].get_id() == &*SKIP_ROUND_CANDIDATE_BLOCKID);
        }
    }

    {
        //check voting

        for i in 0..total_nodes {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Vote(_)),
                "Action should be Vote instead of {:?}",
                &action
            );

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());

            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            if i < 2 * total_nodes / 3 {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                    "Action should be Empty instead of {:?}",
                    &action
                );
            } else {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Precommit(_)),
                    "Action should be Precommit instead of {:?}",
                    &action
                );
            }
        }

        assert!(state.get_hash() == 2802687479);
    }

    {
        //check attempts

        for j in 1..options.max_round_attempts {
            let action = state.create_action(&description, 0, attempt_id + j);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Vote(_)),
                "Action should be Vote instead of {:?}",
                action
            );
        }

        for j in options.max_round_attempts..options.max_round_attempts + 10 {
            let action = state.create_action(&description, 0, attempt_id + j);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                "Action should be Empty instead of {:?}",
                &action
            );
        }
    }

    {
        //check precommits #1 (fast voting)

        let mut state = state.clone();
        let mut attempt_id = attempt_id;

        for i in 0..total_nodes {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            if i <= 2 * total_nodes / 3 {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Precommit(_)),
                    "Action for validator {} should be Precommit instead of {:?}",
                    i,
                    &action
                );
            } else {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                    "Action for validator {} should be Empty instead of {:?}",
                    i,
                    &action
                );
            }

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());

            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                "Action should be Empty instead of {:?}",
                action
            );
        }

        assert!(state.get_hash() == 3568563261);

        attempt_id += 10;

        check_empty_actions(&mut description, &state, attempt_id);

        //check signing candidate

        for i in 0..total_nodes {
            let block = state.choose_block_to_sign(&description, i);

            assert!(block.is_some());
            assert!(block.unwrap().get_id() == &c2);
        }

        //check commits

        for i in 0..2 * total_nodes / 3 {
            let signature = vec![126; 1];
            let action =
                ton::message::Commit { round: 0, candidate: c2.clone(), signature }.into_boxed();

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_current_round_sequence_number() == 0);
        assert!(state.get_hash() == 1464332123);

        for i in 2 * total_nodes / 3..total_nodes {
            let signature = vec![126; 1];
            let action =
                ton::message::Commit { round: 0, candidate: c2.clone(), signature }.into_boxed();

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        //check round switching

        assert!(state.get_current_round_sequence_number() == 1);

        assert!(state.get_hash() == 943640875);

        //check signatures

        let signatures = state.get_committed_block_signatures(0);

        for signature in signatures.get_iter() {
            assert!(signature.is_some());
        }
    }

    {
        //check precommits #2 (slow voting)

        for i in 0..total_nodes / 3 {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Precommit(_)),
                "Action for validator {} should be Precommit instead of {:?}",
                i,
                action
            );

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());

            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                "Action should be Empty instead of {:?}",
                action
            );
        }

        assert!(state.get_hash() == 1629278980);

        attempt_id += options.max_round_attempts - 1;

        loop {
            attempt_id += 1;

            for i in 0..total_nodes {
                let action = state.create_action(&description, i, attempt_id);

                assert!(action.is_some());

                let action = action.unwrap();

                if i < total_nodes / 3 {
                    assert!(
                        matches!(action, ton::Message::ValidatorSession_Message_Vote(_)),
                        "Action for validator {} should be Vote instead of {:?}",
                        i,
                        &action
                    );
                } else {
                    assert!(
                        matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                        "Action for validator {} should be Empty instead of {:?}",
                        i,
                        &action
                    );
                }

                state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
                state = state.move_to_persistent(description.get_cache());

                let action = state.create_action(&description, i, attempt_id);

                assert!(action.is_some());

                let action = action.unwrap();

                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                    "Action should be Empty instead of {:?}",
                    &action
                );
            }

            description.get_cache().clear_temp_memory();

            if description.get_vote_for_author(attempt_id) >= total_nodes / 3 {
                break;
            }
        }

        assert!(state.get_hash() == 4130462109);
    }

    {
        //send new candidate for old round from another source

        let message = ton::message::SubmittedBlock {
            round: 0,
            root_hash: zero_hash.clone(),
            file_hash: zero_hash.clone(),
            collated_data_file_hash: zero_hash.clone(),
        }
        .into_boxed();

        state = state.apply_action(&mut description, 0, attempt_id, &message, now, now);
        state = state.move_to_persistent(description.get_cache());

        assert!(state.get_hash() == 3571838886);
    }

    let mut idx = description.get_vote_for_author(attempt_id);

    {
        //checking votes

        for i in 0..total_nodes {
            assert!(state.check_need_generate_vote_for(&description, i, attempt_id) == (i == idx));
        }
    }

    {
        //check approvals for another candidate

        for i in 0..total_nodes {
            let signature = vec![127; 1];
            let action = ton::message::ApprovedBlock { round: 0, candidate: c1.clone(), signature }
                .into_boxed();

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_hash() == 2876715556);
    }

    {
        //check vote-for generation

        let mut action = state.generate_vote_for(&mut description, idx, attempt_id);

        if let ton::Message::ValidatorSession_Message_VoteFor(ref mut vote_for) = action {
            vote_for.candidate = c1.clone();
        } else {
            unreachable!();
        }

        state = state.apply_action(&mut description, idx, attempt_id, &action, now, now);
        state = state.move_to_persistent(description.get_cache());

        assert!(state.get_hash() == 645596951);

        println!("state: {}", state.dump(&description));
    }

    {
        //check voting

        for i in 0..total_nodes {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            if i < total_nodes / 3 {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                    "Action for validator {} should be Empty instead of {:?}",
                    i,
                    &action
                );
            } else {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Vote(_)),
                    "Action for validator {} should be Vote instead of {:?}",
                    i,
                    &action
                );
            }

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_hash() == 224182806);
    }

    attempt_id += 1;
    idx = description.get_vote_for_author(attempt_id);

    {
        //check voting results

        for i in 0..total_nodes {
            assert!(
                state.check_need_generate_vote_for(&description, i, attempt_id) == (i == idx),
                "check_need_generate_vote_for failed for source {} and attempt {}",
                i,
                attempt_id
            );
        }

        for i in 0..total_nodes {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                "Action for validator {} should be Empty instead of {:?}",
                i,
                &action
            );

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_hash() == 3203236222);
    }

    {
        //check vote-for generation

        let mut action = state.generate_vote_for(&mut description, idx, attempt_id);

        if let ton::Message::ValidatorSession_Message_VoteFor(ref mut vote_for) = action {
            vote_for.candidate = c1.clone();
        } else {
            unreachable!();
        }

        state = state.apply_action(&mut description, idx, attempt_id, &action, now, now);
        state = state.move_to_persistent(description.get_cache());

        assert!(state.get_hash() == 870429773);
    }

    {
        //check voting

        for i in 0..total_nodes {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Vote(_)),
                "Action for validator {} should be Vote instead of {:?}",
                i,
                &action
            );

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_hash() == 1237048486);
    }

    {
        //check precommits

        for i in 0..total_nodes / 3 {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Precommit(_)),
                "Action for validator {} should be Precommit instead of {:?}",
                i,
                &action
            );

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());

            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                "Action for validator {} should be Empty instead of {:?}",
                i,
                &action
            );
        }

        assert!(state.get_hash() == 2558822588);
    }

    attempt_id += 1;
    idx = description.get_vote_for_author(attempt_id);

    {
        //check vote-for generation

        let mut action = state.generate_vote_for(&mut description, idx, attempt_id);

        if let ton::Message::ValidatorSession_Message_VoteFor(ref mut vote_for) = action {
            vote_for.candidate = c1.clone();
        } else {
            unreachable!();
        }

        state = state.apply_action(&mut description, idx, attempt_id, &action, now, now);
        state = state.move_to_persistent(description.get_cache());

        assert!(state.get_hash() == 3437384989);
    }

    {
        //check voting

        for i in 0..total_nodes {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Vote(_)),
                "Action for validator {} should be Vote instead of {:?}",
                i,
                &action
            );

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());
        }

        assert!(state.get_hash() == 193196250);
    }

    {
        //check precommits

        for i in 0..total_nodes {
            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            if i <= 2 * total_nodes / 3 {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Precommit(_)),
                    "Action for validator {} should be Precommit instead of {:?}",
                    i,
                    &action
                );
            } else {
                assert!(
                    matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                    "Action for validator {} should be Empty instead of {:?}",
                    i,
                    &action
                );
            }

            state = state.apply_action(&mut description, i, attempt_id, &action, now, now);
            state = state.move_to_persistent(description.get_cache());

            let action = state.create_action(&description, i, attempt_id);

            assert!(action.is_some());

            let action = action.unwrap();

            assert!(
                matches!(action, ton::Message::ValidatorSession_Message_Empty(_)),
                "Action for validator {} should be Empty instead of {:?}",
                i,
                &action
            );
        }

        assert!(state.get_hash() == 2711711321);
    }
}

fn get_sign_string(signature: &BlockCandidateSignaturePtr) -> String {
    if signature.is_none() {
        return "".to_string();
    }

    match std::str::from_utf8(signature.as_ref().unwrap().get_signature()) {
        Ok(s) => s.to_string(),
        _ => "Error".to_string(),
    }
}

pub fn test_state_hashes_part2(options: &SessionOptions, total_nodes: u32) {
    let mut description = SessionDescriptionImpl::new(options, total_nodes);
    let zero_hash = BlockHash::default();
    let now = std::time::SystemTime::now();

    let sig1 = SessionFactory::create_block_candidate_signature(&mut description, vec![b'a'; 1]);
    let sig2 = SessionFactory::create_block_candidate_signature(&mut description, vec![b'b'; 1]);
    let sig3 = SessionFactory::create_block_candidate_signature(&mut description, vec![b'c'; 1]);
    let sig4 = SessionFactory::create_block_candidate_signature(&mut description, vec![b'd'; 1]);

    {
        let m1 = sig1.merge(&sig2, &mut description);
        assert!(m1.as_ref().unwrap().get_signature()[0] == b'a');
        let m2 = sig2.merge(&sig1, &mut description);
        assert!(m2.as_ref().unwrap().get_signature()[0] == b'a');
    }

    let sig_vec_null: Vec<BlockCandidateSignaturePtr> =
        vec![None; description.get_total_nodes() as usize];
    let mut sig_vec1 = SessionFactory::create_vector(&mut description, sig_vec_null.clone());
    let mut sig_vec2 = SessionFactory::create_vector(&mut description, sig_vec_null.clone());

    sig_vec1 = sig_vec1.change(&mut description, 0, sig1.clone());
    sig_vec1 = sig_vec1.change(&mut description, 1, sig3.clone());
    sig_vec2 = sig_vec2.change(&mut description, 0, sig4.clone());
    sig_vec2 = sig_vec2.change(&mut description, 1, sig2.clone());
    sig_vec2 = sig_vec2.change(&mut description, 2, sig4.clone());

    assert!(sig_vec1.get_hash() == 1072513633);
    assert!(sig_vec2.get_hash() == 2228639731);

    {
        //check vectors merge

        let m1 = sig_vec1.merge(&sig_vec2, &mut description);

        assert!(get_sign_string(m1.at(0)) == "a");
        assert!(get_sign_string(m1.at(1)) == "b");
        assert!(get_sign_string(m1.at(2)) == "d");
        assert!(get_sign_string(m1.at(3)).is_empty());

        let m2 = sig_vec2.merge(&sig_vec1, &mut description);

        assert!(get_sign_string(m2.at(0)) == "a");
        assert!(get_sign_string(m2.at(1)) == "b");
        assert!(get_sign_string(m2.at(2)) == "d");
        assert!(get_sign_string(m2.at(3)).is_empty());

        assert!(m1.get_hash() == 3697200380 && m2.get_hash() == 3697200380);
    }

    let sentb = SessionFactory::create_sent_block(
        &mut description,
        0,
        zero_hash.clone(),
        zero_hash.clone(),
        zero_hash.clone(),
        now,
        now,
    );
    let cand1 =
        SessionFactory::create_block_candidate(&mut description, sentb.clone(), sig_vec1.clone());
    let cand2 =
        SessionFactory::create_block_candidate(&mut description, sentb.clone(), sig_vec2.clone());

    {
        //check candidates merge

        let m1 = cand1.merge(&cand2, &mut description);

        assert!(m1.get_block() == &sentb);
        assert!(get_sign_string(m1.get_approvers_list().at(0)) == "a");
        assert!(get_sign_string(m1.get_approvers_list().at(1)) == "b");
        assert!(get_sign_string(m1.get_approvers_list().at(2)) == "d");
        assert!(get_sign_string(m1.get_approvers_list().at(3)).is_empty());

        let m2 = cand2.merge(&cand1, &mut description);

        assert!(m1.get_block() == &sentb);
        assert!(get_sign_string(m2.get_approvers_list().at(0)) == "a");
        assert!(get_sign_string(m2.get_approvers_list().at(1)) == "b");
        assert!(get_sign_string(m2.get_approvers_list().at(2)) == "d");
        assert!(get_sign_string(m2.get_approvers_list().at(3)).is_empty());

        assert!(m1.get_hash() == 3910441588 && m2.get_hash() == 3910441588);
    }

    {
        //check vote candidates merge

        let mut vote_t1 = SessionFactory::create_vote_candidate(&mut description, sentb.clone());
        for i in 0..description.get_total_nodes() {
            if description.generate_random_usize() % 2 == 0 {
                vote_t1 = vote_t1.push(&mut description, i);
            }
        }

        let mut vote_t2 = SessionFactory::create_vote_candidate(&mut description, sentb.clone());
        for i in 0..description.get_total_nodes() {
            if description.generate_random_usize() % 2 == 0 {
                vote_t2 = vote_t2.push(&mut description, i);
            }
        }

        // merge vote candidates

        {
            let m = vote_t1.merge(&vote_t2, &mut description);
            for i in 0..description.get_total_nodes() {
                assert!(
                    m.check_block_is_voted_by(i)
                        == (vote_t1.check_block_is_voted_by(i)
                            || vote_t2.check_block_is_voted_by(i))
                );
            }
        }

        //create empty vote candidates

        let mut vote1 = SessionFactory::create_vote_candidate(&mut description, None);
        let mut vote1d = SessionFactory::create_vote_candidate(&mut description, sentb.clone());
        let mut vote2 = SessionFactory::create_vote_candidate(&mut description, sentb.clone());
        let mut vote2d = SessionFactory::create_vote_candidate(&mut description, sentb.clone());

        assert!(vote1.get_id() < vote2.get_id());
        assert!(!(vote2.get_id() < vote1.get_id()));

        for i in 0..description.get_total_nodes() {
            if i < description.get_cutoff_weight() as u32 {
                vote1 = vote1.push(&mut description, i);
            } else {
                vote2 = vote2.push(&mut description, i);
            }
            if i < description.get_cutoff_weight() as u32 - 1 {
                vote1d = vote1d.push(&mut description, i);
            } else {
                vote2d = vote2d.push(&mut description, i);
            }
        }

        let mut v = Some(SessionFactory::create_empty_sorted_vector(&mut description));

        v = v.push(&mut description, vote1.clone());
        v = v.push(&mut description, vote2.clone());

        let vote_vec = vec![false; description.get_total_nodes() as usize];
        let prec0_vec = SessionFactory::create_bool_vector(&mut description, vote_vec);
        let prec1_vec = prec0_vec.change(&mut description, 0, true);
        let prec2_vec = prec0_vec.change(&mut description, 1, true);

        let att0_0 = SessionFactory::create_attempt_with_votes(
            &mut description,
            1,
            v.clone(),
            prec1_vec.clone(),
            None,
        );
        let block = att0_0.get_voted_block(&description);
        assert!(block.is_some());
        assert!(block.unwrap().is_none());

        let mut v1d_vec = Some(SessionFactory::create_empty_sorted_vector(&mut description));
        v1d_vec = v1d_vec.push(&mut description, vote1d.clone());

        let att1_0 = SessionFactory::create_attempt_with_votes(
            &mut description,
            2,
            v1d_vec,
            prec0_vec.clone(),
            None,
        );
        let block = att1_0.get_voted_block(&description);
        assert!(block.is_none());

        let mut v2d_vec = Some(SessionFactory::create_empty_sorted_vector(&mut description));
        v2d_vec = v2d_vec.push(&mut description, vote2d.clone());

        let att1_1 = SessionFactory::create_attempt_with_votes(
            &mut description,
            2,
            v2d_vec,
            prec0_vec.clone(),
            None,
        );
        let block = att1_1.get_voted_block(&description);
        assert!(block.is_none());

        let att2_0 = SessionFactory::create_attempt_with_votes(
            &mut description,
            3,
            v,
            prec2_vec.clone(),
            None,
        );
        let block = att2_0.get_voted_block(&description);
        assert!(block.is_some());
        assert!(block.unwrap().is_none());

        {
            let m = att1_0.merge(&att1_1, &mut description);
            let block = m.get_voted_block(&description);
            assert!(block.is_some());
            assert!(block.unwrap().get_id() == sentb.get_id());
        }

        let total_nodes = description.get_total_nodes() as usize;
        let mut first_att_1 = vec![0u32; total_nodes];
        let mut first_att_2 = vec![0u32; total_nodes];
        for i in 0..total_nodes {
            first_att_1[i] = (description.generate_random_usize() % 1_000_000_001) as u32;
            first_att_2[i] = (description.generate_random_usize() % 1_000_000_001) as u32;
        }
        let first_att_1 = Some(SessionFactory::create_vector(&mut description, first_att_1));
        let first_att_2 = Some(SessionFactory::create_vector(&mut description, first_att_2));

        let mut last_precommit0 = vec![0u32; total_nodes];
        last_precommit0[0] = 1;
        last_precommit0[1] = 3;
        let last_precommit0 =
            Some(SessionFactory::create_vector(&mut description, last_precommit0));

        let last_precommit1 = vec![0u32; total_nodes];
        let last_precommit1 =
            Some(SessionFactory::create_vector(&mut description, last_precommit1));

        let mut attempts1 = Some(SessionFactory::create_empty_sorted_vector(&mut description));

        attempts1 = attempts1.push(&mut description, att0_0);
        attempts1 = attempts1.push(&mut description, att1_0);
        attempts1 = attempts1.push(&mut description, att2_0);

        let r1 = SessionFactory::create_round_with_attempts(
            &mut description,
            None,
            0,
            None,
            first_att_1.clone(),
            last_precommit0,
            None,
            sig_vec1,
            attempts1,
        );

        assert!(r1.get_last_precommit(0) == 1);
        assert!(r1.get_last_precommit(1) == 3);

        let mut attempts2 = Some(SessionFactory::create_empty_sorted_vector(&mut description));

        attempts2 = attempts2.push(&mut description, att1_1);

        let r2 = SessionFactory::create_round_with_attempts(
            &mut description,
            None,
            0,
            None,
            first_att_2.clone(),
            last_precommit1,
            None,
            sig_vec2,
            attempts2,
        );

        {
            let m = r1.merge(&r2, &mut description);

            for i in 0..description.get_total_nodes() {
                let att = if m.get_first_attempt(i) == *first_att_1.at(i as usize) {
                    if *first_att_2.at(i as usize) != 0 {
                        std::cmp::min(*first_att_1.at(i as usize), *first_att_2.at(i as usize))
                    } else {
                        *first_att_1.at(i as usize)
                    }
                } else {
                    *first_att_2.at(i as usize)
                };

                assert!(att != 0);
            }

            for i in 0..description.get_total_nodes() {
                if i == 1 {
                    assert!(m.get_last_precommit(i) == 3);
                } else {
                    assert!(m.get_last_precommit(i) == 0);
                }
            }
        }
    }
}

fn myrand() -> f64 {
    let mut rng = rand::thread_rng();
    rng.gen_range(0..=100) as f64 * 0.01
}

fn myrand_range_u32(min: u32, max: u32) -> u32 {
    let mut rng = rand::thread_rng();
    let dist = rand::distributions::Uniform::new_inclusive(min, max);
    use rand::distributions::Distribution;
    dist.sample(&mut rng)
}

fn myrand_range_i32(min: i32, max: i32) -> i32 {
    let mut rng = rand::thread_rng();
    let dist = rand::distributions::Uniform::new_inclusive(min, max);
    use rand::distributions::Distribution;
    dist.sample(&mut rng)
}

pub fn test_consensus_simulation(options: &SessionOptions, total_nodes: u32) {
    let zero_hash = BlockHash::default();
    let now = std::time::SystemTime::now();

    for ver in 0..2 {
        let mut description = SessionDescriptionImpl::new(options, total_nodes);
        let sign_prob = 1.0;
        let submit_prob = 0.8;
        let approve_prob = 0.5;
        let blocks_per_sec_per_node = 0.5;
        let adj_total_nodes = total_nodes + if ver == 0 { total_nodes / 3 } else { 0 };

        let mut states: Vec<Vec<SessionStatePtr>> = vec![Vec::new(); adj_total_nodes as usize];

        let mut ts = description.get_ts();

        let mut virt_state = SessionFactory::create_state(&mut description);
        virt_state = virt_state.move_to_persistent(description.get_cache());

        const ITERATIONS_COUNT: u32 = 10000;

        for _ri in 0..ITERATIONS_COUNT {
            let ts_adj = ts;
            let att = description.get_attempt_sequence_number(ts_adj);
            let virt_x = description.get_vote_for_author(att);
            let mut x = virt_x;

            if !virt_state.check_need_generate_vote_for(&description, virt_x, att) || myrand() < 0.5
            {
                x = myrand_range_u32(0, total_nodes - 1);
            }

            let mut adj_x = x;
            if x + total_nodes < adj_total_nodes && myrand_range_u32(0, 1) == 0 {
                adj_x += total_nodes;
            }

            let mut s = if states[adj_x as usize].len() == 0 {
                SessionFactory::create_state(&mut description)
            } else {
                states[adj_x as usize].last().unwrap().clone()
            };

            for _z in 0..3 {
                let mut y = myrand_range_u32(0, adj_total_nodes - 2);

                if adj_x <= y {
                    y += 1;
                }

                if states[y as usize].len() > 0 {
                    let len = states[y as usize].len() as i32;
                    let mut k = myrand_range_i32(len - 2, len - 1);

                    if k < 0 {
                        k = 0;
                    }

                    s = s.merge(&states[y as usize][k as usize], &mut description);
                    s = s.move_to_persistent(description.get_cache());
                }
            }

            let round = s.get_current_round_sequence_number();

            if description.get_normal_node_priority(x, round) >= 0
                && myrand() <= submit_prob
                && !s.check_block_is_sent_by(x)
            {
                let message = ton::message::SubmittedBlock {
                    round: round as i32,
                    root_hash: zero_hash.clone(),
                    file_hash: zero_hash.clone(),
                    collated_data_file_hash: zero_hash.clone(),
                }
                .into_boxed();

                s = s.apply_action(&mut description, x, att, &message, now, now);
                s = s.move_to_persistent(description.get_cache());
            }

            let vec = s.choose_blocks_to_approve(&description, x);
            if vec.len() > 0 && myrand() <= approve_prob {
                let index = myrand_range_u32(0, (vec.len() - 1) as u32) as usize;
                let block = vec[index].clone();
                let id = block.get_id();
                let sig: Vec<u8> = if block.is_some() { [127; 1].into() } else { Vec::new() };

                let message = ton::message::ApprovedBlock {
                    round: round as i32,
                    candidate: id.clone(),
                    signature: sig,
                }
                .into_boxed();

                s = s.apply_action(&mut description, x, att, &message, now, now);
            }

            let to_sign = s.choose_block_to_sign(&description, x);

            if let Some(block) = to_sign {
                if myrand() <= sign_prob {
                    let id = block.get_id().clone();
                    let sig: Vec<u8> = if block.is_some() { [126; 1].into() } else { Vec::new() };

                    let message =
                        ton::message::Commit { round: round as i32, candidate: id, signature: sig }
                            .into_boxed();

                    s = s.apply_action(&mut description, x, att, &message, now, now);
                }
            }

            if s.check_need_generate_vote_for(&description, x, att) {
                let message = s.generate_vote_for(&mut description, x, att);
                s = s.apply_action(&mut description, x, att, &message, now, now);
            }

            loop {
                let message = s.create_action(&mut description, x, att);
                let stop = matches!(
                    &message,
                    None | Some(ton::Message::ValidatorSession_Message_Empty(_))
                );

                s = s.apply_action(&mut description, x, att, &message.unwrap(), now, now);

                if stop {
                    break;
                }
            }

            let (mut s, made) =
                session_state::get_impl(&*s).make_one(&mut description, x, att, s.clone());
            assert!(!made);

            s = s.move_to_persistent(description.get_cache());

            states[adj_x as usize].push(s.clone());

            if myrand() <= 1.0 / blocks_per_sec_per_node / total_nodes as f64 {
                ts += 1 << 32;
            }

            description.clear_temp_memory();

            virt_state = virt_state.merge(&s, &mut description);
            virt_state = virt_state.move_to_persistent(description.get_cache());
        }

        log::info!("virtual state:\n{}", virt_state.dump(&description));
        log::info!("states:");

        for x in states.iter() {
            if x.len() == 0 {
                log::info!("<EMPTY>");
            } else {
                let s = x.last().unwrap();
                log::info!("round={}", s.get_current_round_sequence_number());
            }
        }

        for i in 0..total_nodes {
            for j in 0..total_nodes {
                if states[i as usize].len() == 0 || states[j as usize].len() == 0 {
                    continue;
                }

                let x = myrand_range_u32(0, (states[i as usize].len() - 1) as u32) as usize;
                let y = myrand_range_u32(0, (states[j as usize].len() - 1) as u32) as usize;
                let s1 = states[i as usize][x].clone();
                let s2 = states[j as usize][y].clone();
                let m1 = s1.merge(&s2, &mut description);
                let m2 = s2.merge(&s1, &mut description);

                assert!(m1.get_hash() == m2.get_hash());

                description.clear_temp_memory();
            }
        }

        let mut x_state = SessionFactory::create_state(&mut description);
        x_state = x_state.move_to_persistent(description.get_cache());

        for i in 0..adj_total_nodes {
            if states[i as usize].len() == 0 {
                continue;
            }

            x_state = x_state.merge(&states[i as usize].last().unwrap(), &mut description);
            x_state = x_state.move_to_persistent(description.get_cache());

            description.clear_temp_memory();
        }

        assert!(x_state.get_hash() == virt_state.get_hash());
    }
}
