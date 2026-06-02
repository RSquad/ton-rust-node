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

use crate::{
    engine_traits::EngineOperations, shard_state::ShardStateStuff,
    validator::consensus::BlockPayloadPtr,
};
use consensus_common::{
    CandidateObservedFlags, EnsureCandidateAvailabilityOptions, ResolverPurpose,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::SystemTime,
};
use ton_block::{error, Block, BlockIdExt, Deserializable, Result};

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

/// Result of [`StateResolverCache::collect_unresolved_chain`] — where the base
/// state for a forward Merkle-apply chain comes from.
pub enum ChainAnchor {
    /// Resolved state found in cache.
    Cached(Arc<ShardStateStuff>),
    /// The base is a finalized + applied block whose full state is loaded from
    /// the engine — either a block already evicted from the cache, or the
    /// finalized block retained by `prune_finalized` (its parent was pruned).
    Engine(BlockIdExt),
    /// No single-parent chain reaches a usable anchor (merge block, or a parent
    /// with no known parent links).
    Unresolvable,
}

/// Single entry in the resolver cache.
///
/// C++ equivalent: internal state kept per candidate in `StateResolverImpl`.
#[allow(dead_code)]
#[derive(Clone)]
pub struct StateResolverEntry {
    pub block_id: BlockIdExt,
    /// Raw block payload paired with its deserialized `Block`, parsed once on
    /// arrival.
    pub data: Option<(BlockPayloadPtr, Block)>,
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
        block: Option<Block>,
    ) {
        let now = SystemTime::now();
        let existed = self.entries.contains_key(&block_id);

        // Keep the deserialized block next to the raw payload so materialization
        // and parent-id extraction never re-parse it. The caller may pass an
        // already-parsed block (e.g. right after local collation) to avoid a
        // second parse; otherwise parse it here. A flag-only follow-up (empty
        // payload, `body_present=false`) yields `None` and must not wipe a body
        // we already have.
        let parsed = if flags.body_present && !data.data().is_empty() {
            block
                .or_else(|| Block::construct_from_bytes(data.data()).ok())
                .map(|block| (data, block))
        } else {
            None
        };
        let parent_ids = parsed.as_ref().and_then(|(_, block)| extract_parent_ids(block));

        self.entries
            .entry(block_id.clone())
            .and_modify(|entry| {
                if parsed.is_some() {
                    entry.data = parsed.clone();
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
                data: parsed,
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

    pub fn try_get_block(&self, block_id: &BlockIdExt) -> Option<Block> {
        self.entries.get(block_id)?.data.as_ref().map(|(_, block)| block.clone())
    }

    /// Walk back from `head_id` via `parent_ids`, collecting the chain of locally
    /// collated blocks with their states.
    pub fn collect_local_chain_from(
        &self,
        head_id: &BlockIdExt,
    ) -> Vec<(Arc<ShardStateStuff>, Block)> {
        let mut chain = Vec::new();
        let mut cursor = Some(head_id.clone());
        while let Some(id) = cursor.take() {
            let entry = match self.entries.get(&id) {
                Some(e) => e,
                None => break,
            };
            if !entry.flags.local_collated {
                break;
            }
            let state = match entry.state.clone() {
                Some(s) => s,
                None => break,
            };
            let block = match self.try_get_block(&id) {
                Some(b) => b,
                None => break,
            };
            chain.push((state, block));
            // Single-parent walk only.
            cursor = match entry.parent_ids.as_ref() {
                Some(parents) if parents.len() == 1 => Some(parents[0].clone()),
                _ => None,
            };
        }
        chain.reverse();
        chain
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
                        data: None,
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
    /// Returns `(chain, anchor)` where:
    /// - `chain`: ordered list (oldest → `block_id`) of ancestors to apply
    ///   forward on top of the anchor.
    /// - `anchor`: where the base state comes from:
    ///   - [`ChainAnchor::Cached`] — resolved state found in cache.
    ///   - [`ChainAnchor::Engine`] — the base is a finalized + applied block
    ///     whose state must be loaded from the engine. This is either a block
    ///     already evicted from the cache, or the finalized block retained by
    ///     `prune_finalized` (recognised by its single parent being absent from
    ///     the cache). The base block is **not** part of the apply chain.
    ///   - [`ChainAnchor::Unresolvable`] — no single-parent chain reaches an
    ///     anchor (merge block or missing parent links).
    ///
    /// C++ equivalent: recursive descent in `StateResolverImpl::resolve()`,
    /// where the `Engine` anchor mirrors `ChainState::from_manager`.
    #[allow(dead_code)]
    pub fn collect_unresolved_chain(
        &self,
        block_id: &BlockIdExt,
    ) -> (Vec<BlockIdExt>, ChainAnchor) {
        let mut chain = Vec::new();
        let mut current = block_id.clone();

        loop {
            // `try_get_state_silent` here so a single external chain walk
            // doesn't inflate `ton_node_resolver_cache_{hit,miss}_total` by
            // one event per ancestor step.
            if let Some(state) = self.try_get_state_silent(&current) {
                chain.reverse();
                return (chain, ChainAnchor::Cached(state));
            }

            let entry = match self.entries.get(&current) {
                Some(e) => e,
                None => {
                    // Walked off the end of the cached chain. The last block
                    // still in the cache — the finalized block retained by
                    // `prune_finalized` — is the base; everything collected
                    // above it applies on top. If nothing was collected,
                    // `current` itself is the base.
                    let anchor = chain.pop().unwrap_or(current);
                    chain.reverse();
                    return (chain, ChainAnchor::Engine(anchor));
                }
            };

            chain.push(current);

            // Single-parent walk only. Multi-parent blocks (shard merges,
            // 2 prev_ids) cannot be materialized from one base state via a
            // single Merkle update, so we deliberately bail out and let the
            // caller fall back to `engine.wait_state()` instead of silently
            // picking `parent_ids[0]` and producing a wrong/failing apply.
            current = match &entry.parent_ids {
                Some(parent_ids) if parent_ids.len() == 1 => parent_ids[0].clone(),
                _ => {
                    chain.reverse();
                    return (chain, ChainAnchor::Unresolvable);
                }
            };
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

    /// Remove all entries with `seq_no < finalized.seq_no` for the same shard.
    ///
    /// Called from `ValidatorGroup::on_block_finalized()`. Once a block is
    /// finalized, its ancestors are guaranteed to be applied by the engine,
    /// so their speculative states are no longer needed. The finalized block
    /// itself is kept so its state can serve as an anchor for the recursive
    /// apply of subsequent (not-yet-finalized) blocks without a DB load.
    pub fn prune_finalized(&mut self, finalized: &BlockIdExt) {
        let before = self.entries.len();
        let pruned_ids: Vec<BlockIdExt> = self
            .entries
            .keys()
            .filter(|block_id| {
                block_id.shard() == finalized.shard() && block_id.seq_no() < finalized.seq_no()
            })
            .cloned()
            .collect();

        for id in &pruned_ids {
            self.entries.remove(id);
            self.materializing.remove(id);
        }

        // Drop waiters by id, not by `entries`: a cache-miss in `wait_prev_state`
        // subscribes before knowing the block, leaving an orphan waiter with no
        // `entries` item that would otherwise never be cleaned up.
        self.waiters
            .retain(|id, _| id.shard() != finalized.shard() || id.seq_no() >= finalized.seq_no());

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

/// Read parent `BlockIdExt`s (`prev_ids`) from a deserialized block's info
/// header. Returns `None` on any read failure.
fn extract_parent_ids(block: &Block) -> Option<Vec<BlockIdExt>> {
    block.read_info().ok()?.read_prev_ids().ok()
}

/// Obtain the parent state for `prev_id`, racing the resolver cache against
/// `engine.wait_state()`. Tries, in order: a synchronous cache hit, an in-memory
/// forward-apply materialization, then an async cache subscription raced against
/// the engine. Lets collation/validation proceed on a notarized-but-unfinalized
/// parent. `purpose` distinguishes collation vs validation repair requests.
pub async fn wait_prev_state(
    cache: &Arc<tokio::sync::Mutex<StateResolverCache>>,
    engine: &Arc<dyn EngineOperations>,
    prev_id: &BlockIdExt,
    purpose: ResolverPurpose,
    wait_timeout_ms: u64,
) -> Result<Arc<ShardStateStuff>> {
    if let Some(state) = { cache.lock().await.try_get_state(prev_id) } {
        metrics::counter!("ton_node_resolver_wait_result_total", "source" => "cache_hit")
            .increment(1);
        return Ok(state);
    }

    if let Some(state) = materialize_prev_state(cache, engine, prev_id, wait_timeout_ms).await {
        metrics::counter!("ton_node_resolver_wait_result_total", "source" => "materialized")
            .increment(1);
        return Ok(state);
    }

    let mut cache_rx = {
        let mut guard = cache.lock().await;
        guard.request_availability(prev_id, purpose);
        guard.subscribe_state(prev_id)
    };
    let cache_wait = async {
        loop {
            if let Some(state) = cache_rx.borrow().clone() {
                return Some(state);
            }
            if cache_rx.changed().await.is_err() {
                return None;
            }
        }
    };

    tokio::select! {
        engine_state =
            engine.clone().wait_state(prev_id, Some(wait_timeout_ms), true) => {
            metrics::counter!("ton_node_resolver_wait_result_total", "source" => "engine")
                .increment(1);
            engine_state
        }
        cache_state = cache_wait => {
            if let Some(state) = cache_state {
                metrics::counter!("ton_node_resolver_wait_result_total", "source" => "cache_async")
                    .increment(1);
                Ok(state)
            } else {
                metrics::counter!("ton_node_resolver_wait_result_total", "source" => "engine_fallback")
                    .increment(1);
                engine.clone().wait_state(prev_id, Some(wait_timeout_ms), true).await
            }
        }
    }
}

/// Materialize the state for `target_id` by walking the resolver cache back to
/// an anchor and applying the intervening blocks' Merkle updates forward.
///
/// Manages the cache lock itself (released across the async applies) and the
/// single-flight materializing marker. Returns `None` when the chain can't be
/// resolved from the cache, leaving the caller to fall back to
/// `engine.wait_state()`. Shared by the collator and the validator.
pub async fn materialize_prev_state(
    cache: &Arc<tokio::sync::Mutex<StateResolverCache>>,
    engine: &Arc<dyn EngineOperations>,
    target_id: &BlockIdExt,
    wait_timeout_ms: u64,
) -> Option<Arc<ShardStateStuff>> {
    let (chain_to_apply, anchor) = {
        let mut guard = cache.lock().await;

        if guard.is_materializing(target_id) {
            return None;
        }

        let (chain_ids, anchor) = guard.collect_unresolved_chain(target_id);

        if chain_ids.is_empty() {
            // Nothing to apply: either the target's own state is cached, or it's
            // absent — let the outer engine.wait_state path load it.
            return match anchor {
                ChainAnchor::Cached(state) => Some(state),
                _ => None,
            };
        }
        if matches!(anchor, ChainAnchor::Unresolvable) {
            return None;
        }

        // Validate every body before claiming the materializing marker: the
        // marker is only released by `store_validated_state`, which won't fire
        // if we early-return on a missing body.
        let mut chain = Vec::with_capacity(chain_ids.len());
        for block_id in chain_ids {
            let Some((_, block)) = guard.try_get_entry(&block_id)?.data.as_ref() else {
                return None;
            };
            chain.push((block_id, block.clone()));
        }
        guard.try_start_materializing(target_id);

        (chain, anchor)
    };

    let mut state = match resolve_chain_anchor(engine, anchor, wait_timeout_ms).await {
        Some(state) => state,
        None => {
            cache.lock().await.finish_materializing(target_id);
            return None;
        }
    };

    let mut resolved = Vec::with_capacity(chain_to_apply.len());
    for (block_id, block) in chain_to_apply {
        state = match apply_candidate_state_update(engine, &block_id, &block, state).await {
            Ok(state) => state,
            Err(err) => {
                log::warn!(target: "simplex_resolver",
                    "failed to materialize resolver state for {block_id}: {err}");
                cache.lock().await.finish_materializing(target_id);
                return None;
            }
        };
        resolved.push((block_id, state.clone()));
    }

    let mut guard = cache.lock().await;
    for (block_id, s) in &resolved {
        guard.store_validated_state(block_id, s.clone());
    }
    guard.finish_materializing(target_id);
    metrics::counter!("ton_node_resolver_materialized_total").increment(resolved.len() as u64);

    Some(state)
}

/// Resolve the base state for a forward-apply chain.
async fn resolve_chain_anchor(
    engine: &Arc<dyn EngineOperations>,
    anchor: ChainAnchor,
    wait_timeout_ms: u64,
) -> Option<Arc<ShardStateStuff>> {
    match anchor {
        ChainAnchor::Cached(state) => Some(state),
        // Finalized + applied block: load its full state from the DB. No
        // download — a finalized state must already be persisted.
        ChainAnchor::Engine(id) => engine
            .clone()
            .wait_state(&id, Some(wait_timeout_ms), false)
            .await
            .map_err(|err| {
                log::debug!(target: "simplex_resolver", "engine anchor load failed for {id}: {err}")
            })
            .ok(),
        ChainAnchor::Unresolvable => None,
    }
}

/// Apply a single block's Merkle update on top of `prev_state`.
async fn apply_candidate_state_update(
    engine: &Arc<dyn EngineOperations>,
    block_id: &BlockIdExt,
    block: &Block,
    prev_state: Arc<ShardStateStuff>,
) -> Result<Arc<ShardStateStuff>> {
    let state_update = block.read_state_update()?;
    let prev_state_root = prev_state.root_cell().clone();
    let engine_for_apply = engine.clone();
    let materialized_block_id = block_id.clone();
    let log_block_id = block_id.clone();

    let (next_state_root, _) = tokio::task::spawn_blocking(move || {
        let fast_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            engine_for_apply
                .db_cells_factory()
                .and_then(|cf| state_update.apply_with_factory(&prev_state_root, &cf))
        }))
        .unwrap_or_else(|_| {
            Err(error!(
                "resolver fast Merkle apply path unavailable for {}; falling back",
                log_block_id
            ))
        });

        match fast_result {
            Ok(result) => Ok(result),
            Err(err) => {
                log::debug!(target: "simplex_resolver",
                    "resolver fast Merkle apply failed for {}: {}. Falling back to apply_for()",
                    log_block_id, err);
                state_update.apply_for(&prev_state_root).map_err(|apply_err| {
                    error!(
                        "cannot apply Merkle update for resolver materialization {}: {}",
                        log_block_id, apply_err
                    )
                })
            }
        }
    })
    .await??;

    ShardStateStuff::from_root_cell(
        materialized_block_id,
        next_state_root,
        #[cfg(feature = "telemetry")]
        engine.engine_telemetry(),
        engine.engine_allocated(),
    )
}

#[cfg(test)]
#[path = "tests/test_state_resolver_cache.rs"]
mod tests;
