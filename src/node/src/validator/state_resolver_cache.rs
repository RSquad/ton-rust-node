/*
 * Copyright (C) 2025-2026 RSquad Blockchain Lab.
 *
 * Licensed under the GNU General Public License v3.0.
 * See the LICENSE file in the root of this repository.
 *
 * This software is provided "AS IS", WITHOUT WARRANTY OF ANY KIND.
 */

//! Speculative state resolver cache for Simplex consensus.
//!
//! # Purpose
//!
//! Enables collation on a notarized-but-unfinalized parent without waiting for
//! the engine to apply that parent. This eliminates the "ghost-parent" deadlock
//! where Simplex progress stalls because the engine hasn't persisted the parent
//! state yet.
//!
//! # C++ reference
//!
//! The C++ implementation uses `StateResolverImpl` (state-resolver.cpp) which
//! recursively applies Merkle updates in-memory via `ChainState::apply()`.
//! `BlockProducerImpl::produce()` calls `state_resolver_->resolve(parent_id)`
//! to obtain the shard state for collation.
//!
//! In Rust, Simplex and the validator run on different threads and don't share
//! consensus state directly. This cache sits on the validator side
//! (`ValidatorGroup`) and receives candidate observations from Simplex via the
//! `on_candidate_observed()` listener callback. Collation and validation tasks
//! look up parent states here before falling back to `engine.wait_state()`.
//!
//! # Async wait semantics
//!
//! `subscribe_state()` returns a `tokio::sync::watch::Receiver<Option<...>>`
//! per `BlockIdExt`. Callers await `rx.changed()` (typically inside
//! `tokio::select!` against `engine.wait_state()`) and read the latest
//! value via `rx.borrow()`. `store_validated_state()` is the producer
//! side: a single `send` on the watch sender wakes every waiter for that
//! block instantly. See `Collator::wait_prev_state_via_engine_or_cache`
//! and `ValidateQuery::wait_prev_state_via_engine_or_cache` for the
//! canonical race pattern.
//!
//! # Chain resolution
//!
//! When asked for a state that has a body but no materialized state, the
//! resolver walks back through parent block IDs (extracted from block data)
//! until it finds a resolved state or reaches the engine-applied frontier.
//! It then requests missing ancestors from Simplex via the `ResolverBackend`
//! trait and applies Merkle updates forward to produce the requested state.
//!
//! # Ownership
//!
//! The cache is owned by `ValidatorGroup` as
//! `Arc<tokio::sync::Mutex<StateResolverCache>>`
//! and passed into collation/validation requests. It is pruned on finalization.

use crate::{shard_state::ShardStateStuff, validator::consensus::BlockPayloadPtr};
use consensus_common::{
    CandidateObservedFlags, EnsureCandidateAvailabilityOptions, ResolverPurpose,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::SystemTime,
};
use ton_block::{BlockIdExt, Deserializable};

/// Trait for resolver-to-session communication (dependency inversion).
///
/// Implemented by `ValidatorGroup` to avoid direct coupling between the
/// resolver cache and the Simplex session. Stored as `Weak<dyn ResolverBackend>`
/// so the cache doesn't prevent session cleanup on group stop.
///
/// C++ equivalent: `StateResolverImpl` has direct access to the pool/session;
/// in Rust we use this indirection because validator and simplex live on
/// different threads.
#[allow(dead_code)]
pub trait ResolverBackend: Send + Sync {
    /// Request Simplex to ensure a candidate body (and optionally its parent
    /// chain) is available. This triggers `requestCandidate` repair on the
    /// Simplex side, which will eventually deliver the body via
    /// `on_candidate_observed()`.
    fn request_candidate_availability(
        &self,
        block_id: BlockIdExt,
        opts: EnsureCandidateAvailabilityOptions,
    );
}

/// Single entry in the resolver cache.
///
/// C++ equivalent: internal state kept per candidate in `StateResolverImpl`.
#[allow(dead_code)]
#[derive(Clone)]
pub struct StateResolverEntry {
    pub block_id: BlockIdExt,
    pub data: BlockPayloadPtr,
    pub collated_data: BlockPayloadPtr,
    pub flags: CandidateObservedFlags,
    /// Parent block IDs extracted from the block data via `read_prev_ids()`.
    /// Populated lazily on first access or on upsert.
    pub parent_ids: Option<Vec<BlockIdExt>>,
    /// Materialized shard state after applying this block's Merkle update.
    /// `None` until validation produces the state.
    pub state: Option<Arc<ShardStateStuff>>,
    pub observed_at: SystemTime,
}

/// Cache of speculative (notarized-but-unfinalized) block states.
///
/// C++ counterpart: `StateResolverImpl` in `state-resolver.cpp`.
pub struct StateResolverCache {
    /// Observed speculative blocks keyed by `BlockIdExt`.
    /// Stores both candidate payloads and resolved state (when available).
    entries: HashMap<BlockIdExt, StateResolverEntry>,
    /// Per-block watch channels for async state notification.
    /// `store_validated_state()` sends on these; consumers obtain a
    /// `watch::Receiver` via `subscribe_state()` and await `rx.changed()`
    /// (typically inside `tokio::select!` against `engine.wait_state()`).
    waiters: HashMap<BlockIdExt, tokio::sync::watch::Sender<Option<Arc<ShardStateStuff>>>>,
    /// Weak reference to the session/validator-group backend for repair requests.
    backend: Option<Weak<dyn ResolverBackend>>,
    /// Single-flight guard: blocks currently being materialized.
    /// Prevents concurrent chain walks for the same target.
    /// C++ equivalent: `CachedState::started` + promise queue.
    materializing: HashSet<BlockIdExt>,
}

impl Default for StateResolverCache {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            waiters: HashMap::new(),
            backend: None,
            materializing: HashSet::new(),
        }
    }
}

impl StateResolverCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the backend for reverse-bridge calls to Simplex.
    ///
    /// Called once by `ValidatorGroup` after the session is created.
    /// Uses `Weak` to avoid preventing group cleanup.
    #[allow(dead_code)]
    pub fn set_backend(&mut self, backend: Weak<dyn ResolverBackend>) {
        self.backend = Some(backend);
    }

    /// Insert or update a candidate observation.
    ///
    /// Called from `ValidatorGroup::on_candidate_observed()` when Simplex
    /// reports a new or updated candidate. Flags are OR-merged so that a
    /// later notarization event adds `parent_ready = true` to an existing
    /// entry that was created with `parent_ready = false`.
    ///
    /// Parent IDs are extracted from block data on insert to enable chain
    /// resolution without re-parsing.
    ///
    /// C++ equivalent: candidate arrival path in `StateResolverImpl`.
    pub fn upsert_observed_candidate(
        &mut self,
        block_id: BlockIdExt,
        data: BlockPayloadPtr,
        collated_data: BlockPayloadPtr,
        flags: CandidateObservedFlags,
    ) {
        let now = SystemTime::now();
        let existed = self.entries.contains_key(&block_id);

        let parent_ids = if flags.body_present { extract_parent_ids(&data) } else { None };
        // Only the body-bearing observation may overwrite the cached payloads.
        // Flag-only follow-ups (e.g. a later `parent_ready=true` callback that
        // passes empty payloads with `body_present=false`) must not wipe a body
        // we already have, otherwise the OR-merged `flags.body_present` stays
        // `true` while `entry.data` becomes empty — silently disabling
        // resolver materialization for this block until pruning.
        let has_body = flags.body_present && !data.data().is_empty();

        self.entries
            .entry(block_id.clone())
            .and_modify(|entry| {
                if has_body {
                    entry.data = data.clone();
                    entry.collated_data = collated_data.clone();
                }
                entry.flags = CandidateObservedFlags {
                    body_present: entry.flags.body_present || flags.body_present,
                    parent_ready: entry.flags.parent_ready || flags.parent_ready,
                    local_collated: entry.flags.local_collated || flags.local_collated,
                };
                if parent_ids.is_some() {
                    entry.parent_ids = parent_ids.clone();
                }
                entry.observed_at = now;
            })
            .or_insert_with(|| StateResolverEntry {
                block_id: block_id.clone(),
                data,
                collated_data,
                flags,
                parent_ids,
                state: None,
                observed_at: now,
            });

        metrics::counter!("ton_node_resolver_observed_total").increment(1);
        metrics::gauge!("ton_node_resolver_entries").set(self.entries.len() as f64);
        log::debug!(
            target: "simplex_resolver",
            "state_resolver_cache.upsert_observed_candidate block_id={} existed={} body_present={} parent_ready={} local_collated={}",
            block_id,
            existed,
            flags.body_present,
            flags.parent_ready,
            flags.local_collated,
        );
    }

    /// Look up a previously stored shard state for `block_id`.
    ///
    /// Returns `Some(state)` if the block was observed AND its state was
    /// materialized by validation. Used as the synchronous fast path —
    /// every external call counts as one cache lookup in
    /// `ton_node_resolver_cache_hit_total` / `ton_node_resolver_cache_miss_total`.
    ///
    /// Internal callers that need the state probe inside a multi-step
    /// algorithm (e.g. parent-chain walks in `collect_unresolved_chain`)
    /// must use [`Self::try_get_state_silent`] instead, otherwise a single
    /// caller-level lookup gets counted as `O(chain_len)` cache events
    /// and the cache-effectiveness metric becomes useless.
    #[allow(dead_code)]
    pub fn try_get_state(&self, block_id: &BlockIdExt) -> Option<Arc<ShardStateStuff>> {
        let result = self.try_get_state_silent(block_id);
        if result.is_some() {
            metrics::counter!("ton_node_resolver_cache_hit_total").increment(1);
        } else {
            metrics::counter!("ton_node_resolver_cache_miss_total").increment(1);
        }
        result
    }

    /// Same as [`Self::try_get_state`] but does **not** touch the
    /// `ton_node_resolver_cache_*_total` counters.
    ///
    /// Reserved for internal multi-probe loops where a single external
    /// "did we have this?" question expands into many internal cache
    /// reads — counting each one would inflate the cache-effectiveness
    /// metric by `O(chain_len)`.
    fn try_get_state_silent(&self, block_id: &BlockIdExt) -> Option<Arc<ShardStateStuff>> {
        self.entries.get(block_id).and_then(|entry| entry.state.clone())
    }

    /// Look up a full resolver entry for `block_id`.
    #[allow(dead_code)]
    pub fn try_get_entry(&self, block_id: &BlockIdExt) -> Option<&StateResolverEntry> {
        self.entries.get(block_id)
    }

    /// Get a watch receiver for async state notification.
    ///
    /// Returns a `watch::Receiver` that will yield `Some(state)` when
    /// `store_validated_state()` is called for this block. The caller
    /// can use this in `tokio::select!` against `engine.wait_state()`.
    ///
    /// If the state is already available, the receiver immediately holds it.
    /// Reuses existing sender to avoid orphaning prior subscribers.
    #[allow(dead_code)]
    pub fn subscribe_state(
        &mut self,
        block_id: &BlockIdExt,
    ) -> tokio::sync::watch::Receiver<Option<Arc<ShardStateStuff>>> {
        let existing_state = self.try_get_state(block_id);

        if let Some(tx) = self.waiters.get(block_id) {
            if let Some(ref state) = existing_state {
                let _ = tx.send(Some(state.clone()));
            }
            return tx.subscribe();
        }

        let init = existing_state;
        let (tx, rx) = tokio::sync::watch::channel(init);
        self.waiters.insert(block_id.clone(), tx);
        rx
    }

    /// Trigger a backend request to ensure a candidate is available.
    ///
    /// Called internally during chain resolution when a parent block body
    /// is missing from the cache. The backend (ValidatorGroup) forwards
    /// this to `SimplexSession::ensure_candidate_available()`.
    #[allow(dead_code)]
    pub fn request_availability(&self, block_id: &BlockIdExt, purpose: ResolverPurpose) {
        let opts = EnsureCandidateAvailabilityOptions { purpose, include_parent_chain: true };

        metrics::counter!("ton_node_resolver_availability_request_total").increment(1);

        if let Some(ref weak) = self.backend {
            if let Some(backend) = weak.upgrade() {
                // `debug` per the AGENTS.md investigation-restore rule:
                // this fires on every cache-driven probe (collation +
                // validation hot paths, before the tokio::select! wait)
                // and gets very noisy under lossy delivery / engine lag.
                log::debug!(
                    target: "simplex_resolver",
                    "state_resolver_cache.request_availability block_id={} purpose={:?}",
                    block_id,
                    purpose,
                );
                backend.request_candidate_availability(block_id.clone(), opts);
                return;
            }
        }

        log::warn!(
            target: "simplex_resolver",
            "state_resolver_cache.request_availability block_id={} backend unavailable",
            block_id,
        );
    }

    /// Store the shard state produced by validation for a given block.
    ///
    /// After `run_validate_query_any_candidate()` successfully applies a
    /// Merkle update, the resulting `next_state` is stored here so that
    /// subsequent collation on this block can use it without engine lookup.
    /// All waiters subscribed via `subscribe_state()` are notified.
    ///
    /// If the entry doesn't exist yet (e.g. validation completed before
    /// `on_candidate_observed` fired), a minimal entry is created as a
    /// defense-in-depth fallback.
    ///
    /// C++ equivalent: `ChainState::apply()` result stored in resolver.
    #[allow(dead_code)]
    pub fn store_validated_state(
        &mut self,
        block_id: &BlockIdExt,
        next_state: Arc<ShardStateStuff>,
    ) {
        let upserted = match self.entries.get_mut(block_id) {
            Some(entry) => {
                entry.state = Some(next_state.clone());
                false
            }
            None => {
                self.entries.insert(
                    block_id.clone(),
                    StateResolverEntry {
                        block_id: block_id.clone(),
                        data: Arc::new(EmptyPayload),
                        collated_data: Arc::new(EmptyPayload),
                        flags: CandidateObservedFlags::default(),
                        parent_ids: None,
                        state: Some(next_state.clone()),
                        observed_at: SystemTime::now(),
                    },
                );
                true
            }
        };

        self.materializing.remove(block_id);

        if let Some(tx) = self.waiters.get(block_id) {
            let _ = tx.send(Some(next_state.clone()));
        }

        metrics::counter!("ton_node_resolver_store_state_total").increment(1);
        log::info!(
            target: "simplex_resolver",
            "state_resolver_cache.store_validated_state block_id={} upserted={} waiters={} state_block_id={}",
            block_id,
            upserted,
            self.waiters.contains_key(block_id),
            next_state.block_id(),
        );
    }

    /// Walk the parent chain starting from `block_id` and collect the
    /// sequence of ancestor `BlockIdExt`s that are in the cache but do
    /// not yet have a materialized state.
    ///
    /// Returns `(chain, base_state)` where:
    /// - `chain`: ordered list from oldest unresolved ancestor to `block_id`
    /// - `base_state`: the state of the first resolved ancestor (or `None`
    ///   if the chain reaches beyond the cache — engine must provide it)
    ///
    /// C++ equivalent: recursive descent in `StateResolverImpl::resolve()`.
    #[allow(dead_code)]
    pub fn collect_unresolved_chain(
        &self,
        block_id: &BlockIdExt,
    ) -> (Vec<BlockIdExt>, Option<Arc<ShardStateStuff>>) {
        let mut chain = Vec::new();
        let mut current = block_id.clone();

        loop {
            // `try_get_state_silent` here so a single external chain walk
            // doesn't inflate `ton_node_resolver_cache_{hit,miss}_total` by
            // one event per ancestor step.
            if let Some(state) = self.try_get_state_silent(&current) {
                chain.reverse();
                return (chain, Some(state));
            }

            let entry = match self.entries.get(&current) {
                Some(e) => e,
                None => {
                    chain.push(current);
                    chain.reverse();
                    return (chain, None);
                }
            };

            chain.push(current.clone());

            match &entry.parent_ids {
                // Single-parent walk only. Multi-parent blocks (shard merges,
                // 2 prev_ids) cannot be materialized from one base state via a
                // single Merkle update, so we deliberately bail out and let the
                // caller fall back to `engine.wait_state()` instead of silently
                // picking `parent_ids[0]` and producing a wrong/failing apply.
                Some(parent_ids) if parent_ids.len() == 1 => {
                    current = parent_ids[0].clone();
                }
                _ => {
                    chain.reverse();
                    return (chain, None);
                }
            }
        }
    }

    /// Mark a block as currently being materialized (single-flight guard).
    ///
    /// Returns `true` if this is the first materialization attempt for the
    /// block; `false` if another task is already materializing it (caller
    /// should subscribe and wait instead of duplicating work).
    ///
    /// C++ equivalent: `CachedState::started` flag.
    #[allow(dead_code)]
    pub fn try_start_materializing(&mut self, block_id: &BlockIdExt) -> bool {
        self.materializing.insert(block_id.clone())
    }

    /// Check whether a block is currently being materialized by another task.
    #[allow(dead_code)]
    pub fn is_materializing(&self, block_id: &BlockIdExt) -> bool {
        self.materializing.contains(block_id)
    }

    /// Release the in-flight materialization marker for a block.
    ///
    /// Idempotent: safe to call when no marker exists. Used by callers that
    /// need to clear the marker on early/error paths where
    /// `store_validated_state()` (which also clears the marker) won't fire —
    /// otherwise the block would be permanently flagged as "in progress" and
    /// `is_materializing()` would short-circuit every retry until pruning.
    #[allow(dead_code)]
    pub fn finish_materializing(&mut self, block_id: &BlockIdExt) {
        self.materializing.remove(block_id);
    }

    /// Remove all entries with `seq_no <= finalized.seq_no` for the same shard.
    ///
    /// Called from `ValidatorGroup::on_block_finalized()`. Once a block is
    /// finalized, all ancestors are guaranteed to be applied by the engine,
    /// so their speculative states are no longer needed.
    pub fn prune_finalized(&mut self, finalized: &BlockIdExt) {
        let before = self.entries.len();
        let pruned_ids: Vec<BlockIdExt> = self
            .entries
            .keys()
            .filter(|block_id| {
                block_id.shard() == finalized.shard() && block_id.seq_no() <= finalized.seq_no()
            })
            .cloned()
            .collect();

        for id in &pruned_ids {
            self.entries.remove(id);
            self.waiters.remove(id);
            self.materializing.remove(id);
        }

        let pruned = before - self.entries.len();
        if pruned > 0 {
            metrics::counter!("ton_node_resolver_pruned_total").increment(pruned as u64);
            metrics::gauge!("ton_node_resolver_entries").set(self.entries.len() as f64);
            log::info!(
                target: "simplex_resolver",
                "state_resolver_cache.prune_finalized finalized={} pruned={} retained={}",
                finalized,
                pruned,
                self.entries.len(),
            );
        }
    }
}

/// Zero-cost payload placeholder for upsert fallback in `store_validated_state`.
struct EmptyPayload;

impl std::fmt::Debug for EmptyPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EmptyPayload")
    }
}

impl consensus_common::BlockPayload for EmptyPayload {
    fn data(&self) -> &consensus_common::RawBuffer {
        static EMPTY: Vec<u8> = Vec::new();
        &EMPTY
    }

    fn get_creation_time(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH
    }
}

/// Extract parent `BlockIdExt`s from raw block data.
///
/// Parses the block BOC and reads `prev_ids` from the block info header.
/// Returns `None` on any parse failure (empty data, malformed block, etc.)
fn extract_parent_ids(data: &BlockPayloadPtr) -> Option<Vec<BlockIdExt>> {
    let bytes = data.data();
    if bytes.is_empty() {
        return None;
    }
    let block = ton_block::Block::construct_from_bytes(bytes).ok()?;
    let info = block.read_info().ok()?;
    info.read_prev_ids().ok()
}

#[cfg(test)]
#[path = "tests/test_state_resolver_cache.rs"]
mod tests;
