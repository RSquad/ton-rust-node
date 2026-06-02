/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

use super::{
    super::consensus::BlockPayloadPtr, ChainAnchor, ResolverBackend, StateResolverCache,
    StateResolverEntry,
};
use consensus_common::{
    BlockPayload, CandidateObservedFlags, EnsureCandidateAvailabilityOptions, RawBuffer,
    ResolverPurpose,
};
use std::{
    sync::{Arc, Mutex},
    time::SystemTime,
};
use ton_block::{Block, BlockIdExt, Serializable, ShardIdent, UInt256};

#[derive(Debug)]
struct MockPayload {
    data: RawBuffer,
    created_at: SystemTime,
}

impl BlockPayload for MockPayload {
    fn data(&self) -> &RawBuffer {
        &self.data
    }

    fn get_creation_time(&self) -> SystemTime {
        self.created_at
    }
}

fn payload(bytes: Vec<u8>) -> BlockPayloadPtr {
    Arc::new(MockPayload { data: bytes, created_at: SystemTime::UNIX_EPOCH })
}

/// A payload carrying a real, parseable block (distinguished by `global_id`).
fn block_payload(global_id: i32) -> BlockPayloadPtr {
    let mut block = Block::default();
    block.set_global_id(global_id);
    payload(block.write_to_bytes().expect("serialize block"))
}

fn block_id(seq_no: u32, shard: &ShardIdent) -> BlockIdExt {
    BlockIdExt {
        shard_id: shard.clone(),
        seq_no,
        root_hash: UInt256::rand(),
        file_hash: UInt256::rand(),
    }
}

struct BackendRecorder {
    requests: Mutex<Vec<(BlockIdExt, EnsureCandidateAvailabilityOptions)>>,
}

impl ResolverBackend for BackendRecorder {
    fn request_candidate_availability(
        &self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    ) {
        self.requests.lock().expect("requests lock poisoned").push((block_id, opts));
    }
}

#[test]
fn upsert_observed_candidate_or_merges_flags() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let id = block_id(10, &shard);
    let mut cache = StateResolverCache::new();

    cache.upsert_observed_candidate(
        id.clone(),
        payload(Vec::new()),
        payload(Vec::new()),
        CandidateObservedFlags { body_present: false, parent_ready: false, local_collated: true },
        None,
    );
    cache.upsert_observed_candidate(
        id.clone(),
        payload(vec![1, 2, 3]), // invalid BOC is fine; parent extraction becomes None.
        payload(Vec::new()),
        CandidateObservedFlags { body_present: true, parent_ready: true, local_collated: false },
        None,
    );

    let entry = cache.try_get_entry(&id).expect("entry must exist");
    assert!(entry.flags.body_present);
    assert!(entry.flags.parent_ready);
    assert!(entry.flags.local_collated);
}

#[test]
fn subscribe_state_returns_empty_receiver_for_unresolved_block() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let id = block_id(1, &shard);
    let mut cache = StateResolverCache::new();

    let rx = cache.subscribe_state(&id);
    assert!(rx.borrow().is_none());
}

#[test]
fn request_availability_dispatches_to_backend_with_parent_chain() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let id = block_id(42, &shard);
    let mut cache = StateResolverCache::new();

    let backend = Arc::new(BackendRecorder { requests: Mutex::new(Vec::new()) });
    let backend_dyn: Arc<dyn ResolverBackend> = backend.clone();
    cache.set_backend(Arc::downgrade(&backend_dyn));

    cache.request_availability(&id, ResolverPurpose::SimplexCollationParent);

    let requests = backend.requests.lock().expect("requests lock poisoned");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].0, id);
    assert_eq!(requests[0].1.purpose, ResolverPurpose::SimplexCollationParent);
    assert!(requests[0].1.include_parent_chain);
}

#[test]
fn collect_unresolved_chain_returns_target_when_no_parents_known() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let id = block_id(7, &shard);
    let mut cache = StateResolverCache::new();

    cache.upsert_observed_candidate(
        id.clone(),
        payload(Vec::new()),
        payload(Vec::new()),
        CandidateObservedFlags::default(),
        None,
    );

    let (chain, anchor) = cache.collect_unresolved_chain(&id);
    assert_eq!(chain, vec![id]);
    assert!(matches!(anchor, ChainAnchor::Unresolvable));
}

#[test]
fn collect_unresolved_chain_orders_from_oldest_to_target() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let root = block_id(10, &shard);
    let parent = block_id(11, &shard);
    let child = block_id(12, &shard);
    let mut cache = StateResolverCache::new();

    let flags =
        CandidateObservedFlags { body_present: true, parent_ready: true, local_collated: false };

    cache.entries.insert(
        root.clone(),
        StateResolverEntry {
            block_id: root.clone(),
            data: None,
            collated_data: payload(Vec::new()),
            flags: flags.clone(),
            parent_ids: Some(Vec::new()),
            state: None,
            observed_at: SystemTime::UNIX_EPOCH,
        },
    );
    cache.entries.insert(
        parent.clone(),
        StateResolverEntry {
            block_id: parent.clone(),
            data: None,
            collated_data: payload(Vec::new()),
            flags: flags.clone(),
            parent_ids: Some(vec![root.clone()]),
            state: None,
            observed_at: SystemTime::UNIX_EPOCH,
        },
    );
    cache.entries.insert(
        child.clone(),
        StateResolverEntry {
            block_id: child.clone(),
            data: None,
            collated_data: payload(Vec::new()),
            flags,
            parent_ids: Some(vec![parent.clone()]),
            state: None,
            observed_at: SystemTime::UNIX_EPOCH,
        },
    );

    let (chain, anchor) = cache.collect_unresolved_chain(&child);
    assert_eq!(chain, vec![root, parent, child]);
    assert!(matches!(anchor, ChainAnchor::Unresolvable));
}

#[test]
fn collect_unresolved_chain_anchors_on_block_whose_parent_was_pruned() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    // `finalized` is the block retained by prune_finalized; its parent (seq 9)
    // was pruned and is absent from the cache.
    let finalized = block_id(10, &shard);
    let pruned_parent = block_id(9, &shard);
    let child = block_id(11, &shard);
    let grandchild = block_id(12, &shard);
    let mut cache = StateResolverCache::new();

    let flags =
        CandidateObservedFlags { body_present: true, parent_ready: true, local_collated: false };

    for (id, parent) in [(&finalized, &pruned_parent), (&child, &finalized), (&grandchild, &child)]
    {
        cache.entries.insert(
            id.clone(),
            StateResolverEntry {
                block_id: id.clone(),
                data: None,
                collated_data: payload(Vec::new()),
                flags: flags.clone(),
                parent_ids: Some(vec![parent.clone()]),
                state: None,
                observed_at: SystemTime::UNIX_EPOCH,
            },
        );
    }

    let (chain, anchor) = cache.collect_unresolved_chain(&grandchild);

    assert_eq!(
        chain,
        vec![child, grandchild],
        "the retained finalized block must be the base, not part of the apply chain"
    );
    match anchor {
        ChainAnchor::Engine(id) => assert_eq!(id, finalized, "base must be the finalized block"),
        _ => panic!("expected Engine(finalized) anchor"),
    }
}

#[test]
fn upsert_observed_candidate_does_not_wipe_body_on_flag_only_followup() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let id = block_id(7, &shard);
    let mut cache = StateResolverCache::new();

    let body = block_payload(42);
    let collated_bytes = vec![9, 8, 7];
    cache.upsert_observed_candidate(
        id.clone(),
        body.clone(),
        payload(collated_bytes.clone()),
        CandidateObservedFlags { body_present: true, parent_ready: false, local_collated: false },
        None,
    );

    cache.upsert_observed_candidate(
        id.clone(),
        payload(Vec::new()),
        payload(Vec::new()),
        CandidateObservedFlags { body_present: false, parent_ready: true, local_collated: false },
        None,
    );

    let entry = cache.try_get_entry(&id).expect("entry must exist");
    assert!(entry.flags.body_present, "body_present must remain OR-merged true");
    assert!(entry.flags.parent_ready, "parent_ready must be merged true");
    let (stored, _block) = entry.data.as_ref().expect("body must not be wiped");
    assert_eq!(stored.data(), body.data(), "body payload must not be wiped");
    assert_eq!(
        entry.collated_data.data(),
        collated_bytes.as_slice(),
        "collated payload must not be wiped"
    );
}

#[test]
fn upsert_observed_candidate_overwrites_payload_when_new_observation_carries_body() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let id = block_id(7, &shard);
    let mut cache = StateResolverCache::new();

    cache.upsert_observed_candidate(
        id.clone(),
        block_payload(1),
        payload(vec![9, 8]),
        CandidateObservedFlags { body_present: true, parent_ready: false, local_collated: false },
        None,
    );

    let new_body = block_payload(2);
    let new_collated = vec![5, 5, 5];
    cache.upsert_observed_candidate(
        id.clone(),
        new_body.clone(),
        payload(new_collated.clone()),
        CandidateObservedFlags { body_present: true, parent_ready: true, local_collated: false },
        None,
    );

    let entry = cache.try_get_entry(&id).expect("entry must exist");
    let (stored, _block) = entry.data.as_ref().expect("body present");
    assert_eq!(stored.data(), new_body.data(), "body must be replaced by new body");
    assert_eq!(
        entry.collated_data.data(),
        new_collated.as_slice(),
        "collated must be replaced by new collated"
    );
}

#[test]
fn collect_unresolved_chain_bails_on_multi_parent_merge_block() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let parent_a = block_id(10, &shard);
    let parent_b = block_id(11, &shard);
    let merge = block_id(12, &shard);
    let mut cache = StateResolverCache::new();

    let flags =
        CandidateObservedFlags { body_present: true, parent_ready: true, local_collated: false };

    cache.entries.insert(
        merge.clone(),
        StateResolverEntry {
            block_id: merge.clone(),
            data: None,
            collated_data: payload(Vec::new()),
            flags,
            // Two prev_ids = shard merge: must not be walked via parent_ids[0].
            parent_ids: Some(vec![parent_a.clone(), parent_b.clone()]),
            state: None,
            observed_at: SystemTime::UNIX_EPOCH,
        },
    );

    let (chain, anchor) = cache.collect_unresolved_chain(&merge);

    assert_eq!(chain, vec![merge], "merge block must be returned without walking either parent");
    assert!(
        matches!(anchor, ChainAnchor::Unresolvable),
        "multi-parent walk must defer to engine, not pick parent_ids[0]"
    );
}

#[test]
fn finish_materializing_clears_in_flight_marker_and_is_idempotent() {
    let shard = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let id = block_id(7, &shard);
    let mut cache = StateResolverCache::new();

    assert!(!cache.is_materializing(&id));

    assert!(cache.try_start_materializing(&id));
    assert!(cache.is_materializing(&id));

    cache.finish_materializing(&id);
    assert!(!cache.is_materializing(&id));

    cache.finish_materializing(&id);
    assert!(!cache.is_materializing(&id));

    assert!(cache.try_start_materializing(&id));
    assert!(cache.is_materializing(&id));
}

#[test]
fn prune_finalized_removes_only_old_entries_in_same_shard() {
    let shard_a = ShardIdent::with_tagged_prefix(0, 0x8000_0000_0000_0000).expect("valid shard");
    let shard_b = ShardIdent::with_tagged_prefix(0, 0x9000_0000_0000_0000).expect("valid shard");
    let id_a_10 = block_id(10, &shard_a);
    let id_a_11 = block_id(11, &shard_a);
    let id_a_12 = block_id(12, &shard_a);
    let id_b_11 = block_id(11, &shard_b);
    let mut cache = StateResolverCache::new();

    for id in [&id_a_10, &id_a_11, &id_a_12, &id_b_11] {
        cache.upsert_observed_candidate(
            id.clone(),
            payload(Vec::new()),
            payload(Vec::new()),
            CandidateObservedFlags::default(),
            None,
        );
    }

    cache.prune_finalized(&id_a_11);

    assert!(cache.try_get_entry(&id_a_10).is_none(), "older ancestor pruned");
    // The finalized block itself is kept as an anchor for forward apply.
    assert!(cache.try_get_entry(&id_a_11).is_some(), "finalized block retained as anchor");
    assert!(cache.try_get_entry(&id_a_12).is_some(), "newer block retained");
    assert!(cache.try_get_entry(&id_b_11).is_some(), "other shard untouched");
}
