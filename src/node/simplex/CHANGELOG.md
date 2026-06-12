# Changelog

All notable changes to the Simplex Consensus Protocol implementation will be documented in this file.

## [Unreleased]

## [0.7.1] - 2026-06-09

Maintenance + parity release: **async DB persistence moved off the SXMAIN
consensus thread**, **restart-recovery hardening** (skip/final-cert replay,
base repair across already-skipped slots, deferred window publication),
**non-fatal invariant handling** (skipscan, finalized-callback dedup,
far-future FinalCert), **requestCandidate repair validation**, **shard
collation timing parity**, and the opt-in **block-sync overlay (observers)**
ingress path. ~40 non-merge commits since v0.7.0.

**C++ baseline**: upstream [ton-blockchain/ton](https://github.com/ton-blockchain/ton)
`testnet/validator/consensus/simplex` at local mirror commit `6655bbdd`
(fast-forwarded 2026-04-24; +19 commits over the v0.7.0 `5cbcc5d3`
baseline). This is the snapshot the `validator-engine` binary is built from
for the mixed Rust/C++ 5x5 simplex network acceptance test
(`node/tests/test_run_net_py/run_test_mixed_5x5_simplex.sh`).

**Milestones**
- The SXMAIN consensus thread no longer blocks on RocksDB latency: every
  SYNC-CRITICAL persist (pool state, our-vote-before-broadcast, the three
  cert handlers, candidate-info waits, and the MC finalized record) is
  handed to a per-session pending-async-DB-results registry and completed
  from a `check_all()` continuation, preserving the persist-before-action
  ordering and matching the C++ db-actor model.
- Restart recovery replays persisted skip and final certificates and
  repairs the progress-cursor `available_base` across already-skipped
  slots before live ingress is accepted — fixing the releasenet stall
  where `slot_np` advanced through skipped slots while `slot_nf` and the
  finalized seqno stayed frozen.
- Invariant breaches that previously panicked the validator (broken
  skipscan boundary, duplicate finalized callback per seqno, far-future
  FinalCert base) now degrade gracefully with bounded fallbacks and
  idempotent guards.

### Added

#### Async DB persistence registry (off SXMAIN `wait()`)

- `SessionProcessor::post_async_db_result(label, result, timeout, on_ready)`
  registers an in-flight async DB result with a one-shot continuation and
  arms `set_next_awake_time(now + ASYNC_DB_POLL_DELAY)` (5 ms cadence) so
  SXMAIN re-polls without busy-waiting.
- `PendingAsyncDbEntry` (`id`, `op_label`, `result`, `registered_at`,
  `deadline`, `on_ready`) and the `pending_async_db_results` registry on
  `SessionProcessor`.
- `process_pending_async_db_results()` drains ready entries via `try_get()`
  from `check_all()` (next to `process_delayed_actions()`), fires
  continuations on Ok/Err, times out past-deadline entries with
  `Err("<label>: db wait timed out")`, and re-arms the wake for still-pending
  entries. Default per-write timeout `DEFAULT_ASYNC_DB_WRITE_TIMEOUT` = 30 s,
  measured on session time (overridable by the test clock).
- New metrics: `simplex_async_db_pending_count` (gauge),
  `simplex_async_db_timeout_total` (counter),
  `simplex_async_db_completion_latency_ms` (histogram).
- `consensus-common`: typed `StorageResultAlreadyTaken` sentinel (with a
  manual `Display` preserving the legacy text) replaces the stringly-typed
  `err.to_string().contains("result already taken")` match — zero-alloc,
  zero false-positive (`downcast_ref::<StorageResultAlreadyTaken>()`).

#### Block-sync overlay (observers) — opt-in, C++ #2380 parity

- `SessionOptions::enable_observers` (default `false`) maps to
  `SimplexConfig.enable_observers` (ConfigParam 30). When enabled, block
  candidates propagate over a dedicated block-sync overlay and candidate
  broadcasts arriving on the consensus private overlay are dropped at
  ingress (mirrors C++ `private-overlay.cpp`).
- `consensus.blockSyncOverlayId` TL type + `utils::compute_block_sync_overlay_short_id(session_id)`.
  The block-sync seed excludes the validator-set node list, so its short id
  differs from the consensus overlay's even for the same `session_id`;
  byte-equal with C++ via `test_block_sync_overlay_id_matches_cpp`.
- New counter `simplex_receiver_in_broadcasts_dropped_observers`.

#### Shard collation timing parity

- `compute_collation_timing()` returns `CollationTiming { dispatch_time,
  min_gen_time, start_collate_before, parent_gen_utime_ms }`. Shard
  collation now dispatches at `slot_start - target_rate` (the block utime
  stays `slot_start`), matching C++ `block-producer.cpp`; masterchain
  dispatch is unchanged. Replaces `compute_collation_start_time()`.
- Per-dispatch INFO `COLLATION_TIMING` log (shard, slot, parent,
  parent_gen_utime, dispatch / min-gen instants, start-collate-before delta)
  for production parity verification.

### Changed

#### Restart / startup recovery

- Replay persisted `SkipCert` and `FinalCert` records
  before restart skip generation, so a restarted validator repairs its
  progress cursor from certificate-backed evidence before accepting live
  ingress. A persisted `FinalCert` is now accepted as parent evidence in
  `get_latest_notarized_candidate_up_to` (finalization implies
  notarization), matching C++ `pool.cpp::advance_present()`.
- `set_available_base_after_restart()` detects a first-non-finalized slot
  that already carries a skip certificate and immediately runs
  `propagate_base_after_skip_cert()` +
  `advance_leader_window_on_progress_cursor()` to carry the seeded base to
  the first non-skipped successor, matching the C++
  base-propagation invariant.
- Leader-window publication is suppressed until startup replay completes,
  preventing restored out-of-order certs from advancing into baseless
  windows. `repair_progress_cursor_base_after_startup_replay()` reconstructs
  `available_base` after replay.

#### Async migration of SYNC-CRITICAL DB callers

- `maybe_store_pool_state`, `persist_our_vote_before_broadcast`,
  `handle_{notarization,skip_certificate,finalization}_reached`,
  candidate-info waits (`ensure_candidate_info_stored`), and
  `maybe_apply_finalized_state` were migrated off blocking `result.wait()`
  to the async-DB registry. The in-memory cursor / local state advances
  synchronously before the persist and all post-persist side effects (vote
  broadcast, cert relay, notar/standstill caching, recursive finalized
  walk) run from the continuation, preserving pre-migration ordering and
  the persist-before-action invariant.
- Session-stop teardown: `SessionProcessor::stop()` drains the registry
  (bounded by `SHUTDOWN_DRAIN_TIMEOUT` = 10 s, including chained
  continuations) and then calls `db.sync(SHUTDOWN_DB_SYNC_TIMEOUT = 30 s)`,
  surfacing shutdown-time DB latency / failures with logging — matching
  C++ `bridge.cpp::destroy_inner()` close ordering. Shutdown drain / sync
  deadlines use wall-clock time (`wall_now()`) so a stuck writer cannot
  park `stop()` forever even under a frozen manual clock.

### Fixed

- **Validator panic on a broken skipscan invariant**:
  `SimplexState::find_next_nonskipped_slot` now returns `Option<SlotIndex>`
  with a bounded forward-scan fallback (`is_slot_skipped_cert` walk) and a
  once-per-session error-log latch instead of `panic!`/`assert!`. Callers
  (`propagate_base_after_notarization`, `propagate_base_after_skip_cert`)
  degrade gracefully on `None`: warn, still advance the progress cursor and
  re-check pending blocks, and (skip-cert path) unconditionally erase the
  stale `skip_intervals` boundary for C++ parity. The skip-interval
  `BTreeSet` fast path mirrors C++ `pool.cpp::next_nonskipped_slot_after()`
  unchanged; the fallback only runs on observed corruption.
- **Validator panic on a duplicate finalized callback per seqno**:
  the seqno-keyed dedup is promoted to `FinalizedSeqnoRecord { slot,
  block_id }` and pruned in lock-step with `finalized_delivery_sent` in
  `cleanup_old_slots`; the duplicate-seqno guard is idempotent on an
  identical `block_id` (re-seeds the slot-keyed dedup and returns) and only
  asserts when a genuinely different block would be delivered for the same
  seqno.
- **Validator panic on a far-future FinalCert**: a `WindowAlloc`
  policy (`BoundedByHorizon` / `VerifiedCertificate`) threaded through
  `ensure_window_exists` / `get_slot_mut` / `find_next_nonskipped_slot` /
  `is_slot_skipped_cert_at` lets verified-certificate paths materialize
  slots beyond `max_acceptable_slot()` (mirrors C++ `state.slot_at()`);
  `apply_final_cert_parent_chain_for_verified_certificate()` writes the
  successor `available_base` before `advance_progress_cursor()` runs,
  eliminating the "base unknown for progress cursor slot" panic.
- **Spurious `result already taken` cert-handler errors**: the per-slot cert
  dedup maps share one in-flight persist handle, so a second registry
  continuation observes the `Taken` sentinel rather than a real failure.
  The three cert handlers now classify it as a benign redundant wake (TRACE,
  no `increment_error()`, no side effects), removing ~200-290 false ERROR
  lines per node per 6-minute soak. Centralized in the free helper
  `is_storage_result_already_taken()`.
- **requestCandidate repair validation**: reject
  `consensus.block` responses with empty inner candidate bytes and require a
  slot-leader candidate signature before repair data is merged into resolver
  state or served from cache; the C++-compatible partial-merge / serving-cache
  behavior for trusted parts is preserved.
- `ensure_candidate_info_stored` combined-wait (candidateInfo + notarCert)
  is now a real runtime guard (log + `increment_error()` + `on_complete(Err)`),
  not a release-build silent fall-through, with the `debug_assert!` retained
  as a dev-time tripwire.

### Internal / refactor

- `SlotWaitPhase`, `SlotDiagnostic`, `WindowDiagnostic` (and `Display for
  SlotWaitPhase`) moved from `session_processor.rs` to `simplex_state.rs`
  as `pub(crate)` siblings of `SimplexState` (Phase 1 of the Simplex architecture rework),
  breaking the `simplex_state` ↔ `session_processor` import cycle. No
  behavioral change.
- `Ed25519KeyOptionFactory` removed; crate crypto helpers refactored off
  `ton_block` (e.g. `KeyId`-based block-sync overlay id computation).

### Tests

- Lib test count grew to ~559 (`#[test]` in `node/simplex/src/`); 16
  integration + 6 doc-tests unchanged.
- New coverage: async-DB registry infrastructure, per-caller migration
  (`maybe_store_pool_state`, vote persist, cert handlers, finalized state),
  shutdown drain incl. chained continuations,
  `is_storage_result_already_taken` benign classification, skipscan
  no-panic fast-path / fallback / caller paths, finalized-callback seqno
  dedup + same-block idempotency + lock-step pruning, far-future FinalCert
  base repair (panic → repaired state), `test_tn1401_restart_base_repair_crosses_already_skipped_slot`,
  startup skip/final-cert replay + progress-cursor base repair,
  requestCandidate repair body/signature validation, and
  `test_block_sync_overlay_id_matches_cpp`.

### Documentation

- README version bumped to 0.7.1; `Current semantics` extended with the
  async-DB registry, restart-recovery base repair, and the opt-in block-sync
  overlay; `SessionOptions` table gains `enable_observers`; the metrics
  catalog gains the async-DB and observers families; the utils table gains
  `compute_block_sync_overlay_short_id`.
- `Cargo.toml` / `Cargo.lock` crate version bumped `0.7.0` → `0.7.1`.

## [0.7.0] - 2026-04-21

Major release: **state resolver for ghost-parent collation**, **certificate
durability + ordering parity with C++**, **bootstrap-deadlock fixes**,
**bad-signature peer-ban DoS hardening**, **per-session Prometheus
republishing**, and a battery of C++ collation/timeout parity work. ~60
non-merge commits since v0.6.0.

**C++ baseline**: upstream [ton-blockchain/ton](https://github.com/ton-blockchain/ton)
`testnet/validator/consensus/simplex` at local mirror commit `5cbcc5d3`
("Update changelogs", 2026-04-06). This is the snapshot the
`validator-engine` binary was built from for the mixed Rust/C++ 5x5
simplex network acceptance test that gates the v0.7.0 release (see
`node/tests/test_run_net_py/run_test_mixed_5x5_simplex.sh`).

**Milestones**
- Validator-side `StateResolverCache` eliminates the MC collation deadlock
  on a notarized-but-unfinalized parent by materializing parent states
  from cached candidate Merkle updates and racing them against
  `engine.wait_state()`.
- Certificate persistence is now ordered before state transitions and the
  cert-DB schema is unified under `db.key.vote` + `db.cert` matching the
  C++ `simplex-work` model.
- Bootstrap genesis flow no longer deadlocks on the speculative MC
  parent: explicit Simplex session parents, empty-block parity with the
  parent state, MC seqno-lag tolerance.
- `BadSignatureBanState` mirrors C++ `pool.cpp::ban` end-to-end: ingress
  drop for vote/cert/broadcast from banned peers and cert-verify-failure
  -> ban.
- New `prometheus_publisher` module bridges per-session `MetricsHandle`
  dumps to the global `metrics_exporter_prometheus` recorder, with
  selectable label cardinality via `PrometheusLabels`.

### Added

#### State resolver bridge (validator-side `StateResolverCache`)

- `SimplexSession::ensure_candidate_available(block_id, opts)`:
  resolver-driven repair entry point. Posts to the main task queue so
  the body / parent chain can be requested without blocking the
  validator side.
- `SessionListener::on_candidate_observed(block_id, data, collated_data,
  flags)` (in `consensus-common`): every observed candidate is forwarded
  to the validator side so the resolver cache can populate parent
  chains and serve collation/validation without waiting for the engine
  to apply the parent.
- `EnsureCandidateAvailabilityOptions` + `ResolverPurpose`
  (in `consensus-common`): typed contract for resolver probes
  (`SimplexCollationParent` / `SimplexValidationParent`), with
  `include_parent_chain` to fan out the request through the cached
  parent chain.
- `CandidateObservedFlags { body_present, parent_ready, local_collated }`
  re-exported at the crate root for resolver consumers; the simplex
  side sets these whenever a candidate is observed.
- `simplex_resolver` log target for end-to-end resolver tracing
  (debug-level by default; promoted to info on cache misses with state
  changes).

#### Bad-signature peer-ban

- `BadSignatureBanState` in `Receiver`: tracks per-peer ban expiry,
  drops vote / cert / broadcast / requestCandidate ingress from banned
  peers, and fires `Receiver::ban_source_for_bad_signature` on cert
  signature verification failure (mirrors C++ `pool.cpp::ban`).
- `SessionOptions::bad_signature_ban_duration` (default 5 s) plumbed
  through `ReceiverWrapper::create` and surfaced in
  `simplex_config_v2` so the on-chain noncritical-params block can
  tune it without a node restart.

#### Per-session Prometheus republisher

- New `prometheus_publisher` module: walks each session's local
  `MetricsHandle` snapshot and republishes the
  `ton_node_simplex_*` series to the global recorder backing the
  node's `/metrics` endpoint. `*.speed` derivative keys are dropped
  (Prometheus computes rates with `rate()` / `irate()`).
- `SessionOptions::prometheus_labels`: `PrometheusLabels::ShardOnly`
  (default, lowest cardinality) or `PrometheusLabels::ShardAndSessionId`
  (per-session breakdown using the `sid8` prefix from log dumps).
- Wired into the existing 15 s / 30 s metrics dump cadence so the
  warmup-aware first-dump suppression carries over.

#### Collation flow + self-collation metrics

- Per-window collation flow logs at info: leader identity, slot phase,
  collation entry/exit, and self-collation hit/miss counters.
- `simplex_self_collated_*` counter family for tracking when the
  local validator served as the collation source (vs accepting a
  remote candidate) — drives the simnet "self-only collation" health
  finding.

#### MC finalization parity

- Recursive MC finalization parity with C++ `pool.cpp::handle_finalized_block_recursive`.
- ValidatorGroup timeouts surfaced through `SessionOptions` so the
  validator manager can cap collation waits and fall back cleanly.

### Changed

#### Cert order + durability

- All certificate handlers (`set_notarize_certificate`,
  `set_finalize_certificate`, `set_skip_certificate` and their
  observed/saved variants) now wait for the cert to be persisted to
  the DB before triggering any state transition or network side effect
  (broadcast vote, FSM transition). Matches C++ PR #2201
  (`1194034a`): "persist certificates before acting on them".
- The cert DB layout was unified under the `db.key.vote` + `db.cert`
  schema mirroring C++ `simplex-work` previews (`c53c2af8`,
  `8e79a230`). `notarCert`/`finalCert`/`skipCert` records are now a
  single `Certificate` schema; bootstrap recovery walks the unified
  cert table once.
- Wait-for-store semantics added to vote replay during startup: the
  pool will not transition past a slot until every persisted vote /
  cert for it is materialised.
- Standstill replay: receiver cert dedup, vote prefilter, tighter
  replay range against the C++ tracked-slot interval.

#### Collation / timeout C++ parity

- Window-scoped collation cancellation (mid-window leader change no
  longer leaves orphaned collation futures running).
- Skip-safe precollation: precollated candidate is dropped when the
  slot it anchors is skipped via FSM, preventing post-skip publish.
- Progress-anchored timeouts: alarm rearming uses
  `first_non_progressed_slot` as the anchor, matching C++
  `consensus.cpp::set_timeouts`.
- Late same-window candidate publish: candidates produced after the
  initial window deadline but still within the leader window are
  published instead of dropped (C++ `pool.cpp` behaviour).
- Suppress stale finalized rebroadcasts (don't re-broadcast a
  finalization we already applied locally).
- Drop legacy timeout backoff knobs (`timeout_increase_factor`,
  `max_backoff_delay`) from `SessionOptions`; behaviour is now driven
  by `first_block_timeout_multiplier` + `_cap` from
  `simplex_config_v2`.

#### RequestCandidate hardening

- Pacing + parity checks on the receiver-side `requestCandidate`
  retry driver: backoff, peer rotation, partial-update forwarding
  through the resolver bridge.
- Per-peer 1-second sliding-window rate limit
  (`candidate_resolve_rate_limit`) on inbound `requestCandidate`
  queries.

#### Bootstrap deadlock

- Explicit Simplex session parents replace the implicit
  `pipeline_context` dependency: `SimplexState::add_explicit_parent`
  on session start so the genesis flow never has to back-derive
  parents from a half-applied MC state.
- Empty-block generation is resolved from the parent state
  (`should_generate_empty_block`/`create_empty_block_desc` consult
  the parent's normal tip via `SimplexState`), not from a
  pipeline-level cache.
- MC applied-top vs local-finalized progress cleanly separated:
  MC `seqno=1` no longer suppresses empty-block recovery for MC
  `seqno=2`.
- Tolerate bootstrap MC seqno lag in the speculative parent flow:
  `Engine::get_shard_blocks` returns the actual MC seqno via
  `Option<&mut u32>` and the collator falls back to an empty
  shard-blocks view when the speculative parent is ahead of the
  applied MC.

#### Resolver hot-path log discipline

- `StateResolverCache::upsert_observed_candidate`,
  `SimplexSession::ensure_candidate_available`, and
  `ValidatorSessionListener::on_candidate_observed` are all `debug`
  now (were `info` during the simnet investigation). Per the
  workspace investigation-restore rule.

### Fixed

- **MC collation deadlock** on notarized-but-unfinalized parent — the
  resolver cache materializes the missing state in-process instead of
  waiting for the engine to apply the parent.
- **Resolver `materializing` marker leak**: every early-return path
  out of `try_materialize_prev_state_from_cache` now releases the
  marker via `StateResolverCache::finish_materializing` (idempotent).
  Without this, a single failed materialization permanently disabled
  the resolver for a target until pruning.
- **Resolver multi-parent walk**:
  `StateResolverCache::collect_unresolved_chain` no longer silently
  picks `parent_ids[0]` for shard-merge blocks — it now bails out on
  any `parent_ids.len() != 1` and lets the caller fall through to
  `engine.wait_state()`.
- **Resolver body wipe on flag-only upsert**:
  `upsert_observed_candidate` only overwrites `entry.data` /
  `entry.collated_data` when the new observation actually carries a
  body, so a later flag-only callback (e.g. `parent_ready=true` with
  empty payloads) cannot wipe a previously-cached body while
  OR-merging `flags.body_present=true`.
- Genesis collation/validation alignment with C++: zerostate bootstrap
  validation flow no longer blocks waiting for a notarized parent that
  will never arrive.
- Hold progress until skip base is known: `propagate_base_after_skip_cert`
  now defers the `advance_present()` equivalent until the
  `available_base` for the skipped slot is observable, matching C++
  `pool.cpp` behaviour.
- Quick fix for bad election ID on first elections.
- Suppress t0 `low_activity` health finding by delaying the first
  metric dump and health check past the warmup window.
- Stale `wait_state()` reference in the `state_resolver_cache` module
  docs replaced with the actual `subscribe_state()` →
  `tokio::sync::watch::Receiver` flow.

### Removed

- Legacy timeout backoff knobs (`timeout_increase_factor`,
  `max_backoff_delay`) from `SessionOptions`. Replaced by the
  noncritical-params `first_block_timeout_multiplier` +
  `first_block_timeout_cap`.

### Tests

- `test_simplex_consensus_ghost_parent_resolver_probe`: ghost-parent
  integration probe driving the resolver cache through Simplex →
  validator → collation.
- `test_state_resolver_cache.rs`: full unit suite for
  `StateResolverCache` covering upsert flag merging, body-wipe
  prevention, multi-parent bail, materializing marker idempotency,
  unresolved-chain walking, and shard-scoped pruning.
- `test_validator_group.rs`: extracted from `test_collator.rs` into
  its own file; resolver cache wiring + backend bridge tests added.
- `test_prometheus_publisher.rs`: snapshot republishing with mocked
  global recorder, parallel-safe (test-local recorder via thread
  storage), label strategy assertions for both `ShardOnly` and
  `ShardAndSessionId`.
- `test_simplex_state.rs` extended for skip-interval bookkeeping,
  cursor + base alignment, conflicting-vote handling.
- `test_session_processor.rs` heavily extended (~2.7k new lines)
  for cert-order, db-wait-order, bootstrap-deadlock, MC
  finalization, and standstill parity.

### Upstream merges

- `feature-boc-speedup-2` (`3df0ea8b6` + `b532d9086` + `7757f86e5`):
  optimised `BocReader` / `BocWriter` and `Cell`, switched SHA to
  `openssl`, off-by-one fix in `Cell::read_boc_draft`, env-var
  hygiene in `test_boc.rs`, redundant `openssl-sys` direct dep
  removed.
- Two `master` merges pulled in `Remove append_builder usage`
  (PR #815) and `Node build with native flag (action)` (PR #821).

### Documentation

- README updated: version bumped to 0.7.0, `Current semantics` section
  refreshed for the resolver bridge + bad-signature ban, parity gap
  list trimmed (state-resolver / ghost-parent / cert-order / db-wait-
  order / DOS-hardening moved to `Resolved`).
- Crate-level `lib.rs` doc clarifies the new `on_candidate_observed`
  callback in the `SessionListener` integration list.
- `state_resolver_cache.rs` module doc rewritten to describe
  `subscribe_state()` → `watch::Receiver` (the previously-described
  `wait_state()` API never existed in this crate).
- `prometheus_publisher.rs` module doc explains why republishing is
  used instead of switching every increment site to the global
  recorder.

### Follow-ups

- Secondary index for `SimplexDb` cert lookups by `candidate_id` /
  `slot` to keep `load_*_by_id` / `load_skip_cert_by_slot` O(1) after
  the cert-storage consolidation (currently O(n) prefix-scan +
  per-entry deserialize).

## [0.6.0] - 2026-04-08

Major release: **finalized-driven delivery**, **C++ parity overhaul**, **legacy mode removal**,
and **production-grade stall diagnostics**. 71 commits since v0.5.0.

**Milestones**
- Simplex switched to finalized-driven delivery (`on_block_finalized()`); the old sequential
  `on_block_committed()` path is no longer used by Simplex.
- Legacy fallback/strict-parent mode removed; only C++-compatible three-vote behavior remains.
- Structured stall-diagnosis debug dump with health findings, per-slot phase tracking,
  and per-peer activity snapshots.

### Added

#### Finalized-driven delivery
- **`handle_block_finalized()`**: delivers finalized blocks through `SessionListener::on_block_finalized()`
  as soon as a `FinalCert` is observed and the block body is available.
- **`maybe_apply_finalized_state()`**: updates local finalized-head state (seqno, block ID) after delivery.
- **`finalized_pending_body`**: when a finalization certificate arrives before the candidate body,
  the finalized entry is stored and later materialized when the body arrives.
- **Out-of-order finalized delivery** is the only mode; the old `SessionOptions::out_of_order_finalized_delivery` toggle removed.

#### Session lifecycle
- **Separate session creation from start**: `SessionFactory::create_session()` returns a session handle;
  `session.start(initial_block_seqno)` begins consensus processing with the expected first block seqno.
- **`simplex_config_v2` deserialization**: unified `SimplexConfig` (v1+v2), named
  `NoncriticalParams` struct (13 fields), flat JSON. `SIMPLEX_USE_TESTING_CONSTANTS` removed.

#### C++ parity features
- **Candidate chaining within leader windows**: candidates in a multi-slot leader window build
  on the previous slot's candidate, matching C++ `pool.cpp` behavior.
- **Notarized-parent collation mode** with robust retry: collation selects notarized parents
  as base, with fallback retry on state unavailability.
- **Stale leader window guards** for collation: prevents generation for outdated windows.
- **Bootstrap skip-cascade prevention**: timeouts are unarmed by default during bootstrap,
  preventing spurious skip-vote storms on session startup.
- **Available-base propagation on leader window advancement**: FSM correctly propagates
  the available base when leader windows advance due to skip certificates.
- **Empty candidate FSM-tip validation**: empty candidates are rejected unless the
  referenced block matches the parent state's current normal tip.
- **Leader window desync margin**: `max_leader_window_desync` wired into `SimplexState`/`Receiver`
  ingress checks to limit horizon of accepted slots.
- **Timing wake discipline and pacing parity**: `min_block_interval_ms`
  wired through config → session → runtime pacing. `gen_utime_ms` threaded into candidates/chain-heads.
  Validation waits for `parent.gen_utime_ms + min_block_interval`.

#### Observability and diagnostics
- **Stall dump redesign**: `debug_dump()` rewritten with structured sections:
  - **Conclusion** (stalled only): structured `HealthFinding`s explaining the stall reason.
  - **Header**: shard, validators, local index, total/th66/th33/active weight.
  - **Frontiers**: `first_non_finalized` / `first_non_progressed` with "unchanged for Xs" tracking.
  - **Heads**: `finalized_head_seqno`, `finalized_head` block ID, `last_mc_applied` block ID,
    `last_mc_finalized_seqno`.
  - **Milestone timestamps**: `last_finalization`, `last_notarization`, `last_final_cert`,
    `last_notar_cert` — all with relative time and slot.
  - **Statistics**: candidate funnel (`received/validated/notarized/finalized/other` with %),
    traffic counters, typed vote breakdown (`notar/final/skip`), duplicate counters.
  - **Collation**: grouped by windows with leader identity (validator index + full base64
    `pubkey_hash` + `adnl_id`), slot phase, timing, generated blocks info.
  - **Validation inventory**: lifecycle buckets (`received`, `validated`, `notarized`,
    `finalized` last 10s, `other` with total count), block IDs for correlation with collator,
    per-block percentage of total candidates.
  - **Peers**: per-validator dump with full base64 ADNL ID and pubkey hash, weight in %,
    last activity/vote/cert/candidate times, typed vote/cert counts, candidate counts.
  - **Health findings**: all applicable `HealthFindingKind` anomalies with severity and summary.
- `SessionObservability` struct tracking cursor-change timestamps, last notarization/finalization
  times, last certificate times, and last MC applied block ID.
- `HealthFinding` / `HealthFindingKind` types for structured stall diagnosis in dump header.
- `SlotWaitPhase` / `SlotDiagnostic` / `WindowDiagnostic` types for per-slot and per-window
  structured diagnostics from `SimplexState::collect_window_diagnostics()`.
- `ReceiverActivitySnapshot` / `SourceActivitySnapshot` for comprehensive per-peer network
  activity statistics passed from `Receiver` to `SessionProcessor` via `on_activity()`.
- `CandidateTotals` helper for computing and formatting candidate lifecycle bucket percentages.
- Peer-delivered candidate ingress counters
  (`simplex_candidate_received_broadcast`, `simplex_candidate_received_query`) and a unified
  `simplex_collation_starts` counter, with derivative `/s` dump support for operator debugging.
- Info-level metrics dumps with execution time guards.
- Log throttling and validator isolation detection.
- Skip-dominance health false-positive reduction.
- `simplex_finalized_pending_body_count` gauge for finalized blocks waiting for body arrival.

#### Networking
- Overlay response size increased to +1MB for `requestCandidate` (C++ parity).
- Two-step broadcast TL schema alignment for C++/Rust interop: `extra:bytes` in
  `broadcastTwostepSimple`/`Fec`, `data_size`+`extra` in `broadcastTwostep.id`,
  `consensus.broadcastExtra(slot)` propagated through send path.
- Standstill replay aligned with C++ parity: range tracking, sparse iteration, receiver
  ingress bounds synced with finalized frontier.
- Standstill warning suppression for inactive sessions.
- Receiver ingress bounds synced with finalized frontier.

### Changed
- **Finalized-driven delivery model**: Simplex delivers finalized blocks through
  `on_block_finalized()`. The old `on_block_committed()` sequential path, `try_commit_finalized_chains()`,
  `collect_gapless_commit_chain()`, `commit_single_block()`, and related proof-fetch/retry flow
  are all removed.
- **Restart behavior is state restoration only**: `RestartRecommitStrategy` removed. Startup
  restores finalized state and continues from there without historical replay callbacks.
- `ReceiverListener::on_activity()` now accepts `ReceiverActivitySnapshot` parameter
  for per-peer statistics (breaking internal trait change).
- `SourceStats` in `receiver.rs` extended with typed vote/cert/candidate counters,
  last-receive timestamps, and duplicate counters.
- Health status line format updated: `Session <id> health [OK|STALLED]: shard=... slot_nf=...
  slot_np=... finalized_head_seqno=...`.
- `check_all()` ordering: validated candidates are now processed before FSM timeouts,
  preventing stale-slot validation when timeouts would advance the window first.
- Candidate resolver behavior aligned with C++ parity: DB-backed fallback on cache miss,
  merged partial-response completion (body + notar).
- Certificate relay aligned with C++ `handle_saved_certificate` semantics.
- Parent gating aligned with C++ flow: `is_wait_for_parent_ready()` mirrors C++
  `pool.cpp::maybe_resolve_request()` exactly (finalized-boundary check, notarized-parent
  hash match, skip-gap coverage).
- Progress cursor and repair flow aligned with C++ parity.

### Fixed
- **Stall dump never fired**: `round_debug_at` was never scheduled as a `next_awake_time`,
  so when consensus stalled and no FSM timeouts remained, `check_all()` never ran and the
  stalled dump (at ERROR level) was unreachable. Fixed by adding
  `self.set_next_awake_time(self.round_debug_at)` in `check_all()`.
- **`check_all()` ordering**: process validated candidates before FSM timeouts to prevent
  stale-slot validation when timeouts advance the window.
- **Bootstrap skip cascade**: unarmed-by-default timeouts prevent spurious skip-vote
  storms on session startup.
- **Available-base propagation** on leader window advancement: prevent collation
  deadlock when base is not forwarded after skip certificates.
- **Empty-tip validation**: reject empty candidates whose referenced block does
  not match the parent state's normal tip.
- **First-pending-candidate-wins guard**: prevent notarization race when multiple candidates
  arrive for the same slot; remove `its_over` gate from `try_notar`.
- **Candidates stored as pending despite local skip vote**: candidates arriving after
  the local validator has skip-voted are still stored for potential notarization.
- **Simplex C++ parity fixes**: MC validation ordering, empty block ownership, collation
  gating, MC notification uses `BlockIdExt`, shard empty-block recovery, fixed-base
  timeout schedule, genesis-parent validation, startup timeout gating, pending block
  preservation, and vote-mix observability.
- **Gapless commit scheduler hardening** : monotonic seqno enforcement, resolver cache
  deserialization-failure purge, missing-body-logged cleanup, DB payload lookup order fix,future-time warning.
- **Mid-session finalization parity**: candidate repair completion and DB persistence
  aligned with C++.
- **Cursor ingress alignment** with C++ parity: progress cursor, repair flow, and
  standstill replay all follow C++ progress-cursor semantics.
- **Replay integrity hardening**: deterministic replay order in recovery paths.

### Removed
- **Legacy fallback and strict-parent mode**: `NotarizeFallback`, `SkipFallback` vote types,
  `enable_fallback_protocol` option, `require_finalized_parent` strict mode, and all related
  code paths removed. Only C++-compatible three-vote behavior (Notarize, Finalize, Skip) remains.
- **`get_approved_candidate` callback**: Rust no longer uses `SessionListener::get_approved_candidate`
  and instead relies on the DB-backed repair path.
- **`SessionListener::get_committed_candidate`**: removed together with `CommittedBlockProof` and
  the old missing-proof recovery path.
- **Sequential commit path**: `try_commit_finalized_chains()`, `collect_gapless_commit_chain()`,
  `commit_single_block()`, `notify_block_committed()`, and related proof-fetch/retry flow removed.
- **`RestartRecommitStrategy`**: removed; restart is now state-restoration only.
- **`out_of_order_finalized_delivery` toggle**: removed (out-of-order is the only mode).
- **`DISABLE_NON_FINALIZED_PARENTS_FOR_COLLATION` dead mode**: removed.
- **Empty-parent metadata queue**: `PendingParentResolution` queue, `is_fully_resolved` metadata
  flag, and `ParentAging` health check removed; parent resolution is now on-demand.
- **`requestCandidate2` / `candidateAndCert2`**: already removed in 0.5.0; all remaining
  references cleaned up.

## [0.5.0] - 2026-03-20

### Added
- Download committed block via full-node proof for MC gap recovery.
  Replaces Rust-only `requestCandidate2` with C++-compatible mechanism.
  `SessionListener::get_committed_candidate` trait method, `CommittedBlockProof` type,
  `ValidatorGroup::on_get_committed_candidate` implementation using `download_block_proof()`.
- `test_simplex_consensus_finalcert_recovery`: FinalCert-recovery gremlin test with per-node
  lossy overlay targeting (7 MC nodes, node 0 gets 40% broadcast + 30% message/query loss).
- `lossy_overlay_node_indices` field in `LossyOverlayOpts` for per-node loss targeting.
- C++-parity standstill slot-grid dump (`standstill_slot_grid_dump()`
  on `SimplexState`). Mirrors C++ `pool.cpp::alarm()` per-validator markers (F/I/N/S/.)
  and cert flags (notar/skip/final). Wired into `debug_dump()` on stall detection.
- Receiver-side anomaly checks with configurable thresholds.
  Shared `ReceiverHealthCounters` (`Arc<AtomicU64>`) for cross-thread standstill trigger
  and candidate giveup counting. Delta-based anomaly detection in `run_health_checks()`
  for cert verify failures, standstill triggers, and candidate giveups with cooldown.
  New `SessionOptions` fields: `health_alert_cooldown`, `health_stall_warning_secs`,
  `health_stall_error_secs`, `health_parent_aging_warning_secs`,
  `health_parent_aging_error_secs`.

### Changed
- Restart recovery full replay is now commit-path only in roundless mode: no `on_block_skipped` callbacks are emitted during recommit.
- `RestartRecommitStrategy::FullReplay` now enforces strict deterministic replay invariants (parent-chain continuity, slot monotonicity, and seqno consistency) and fails startup recovery on malformed persisted history.
- `RestartRecommitStrategy::FirstCommitAfterFinalized` now restores state without emitting historical replay actions.
- Removed `RestartRecommitStrategy::FilteredPrependSkips`; `FullReplay` and `FirstCommitAfterFinalized` remain.

### Fixed
- Candidate signature payload now signs bare `consensus.candidateId` directly,
  matching C++ testnet behavior. Previously signed `candidateParent(candidateId(...))` which broke
  Rust/C++ interop for block candidate acceptance. Added regression test
  `test_candidate_id_to_sign_is_bare_candidate_id`.
- MC fork prevention: reject masterchain candidates building on stale heads
  (C++ `block-validator.cpp` parity). Track `last_accepted_mc_seqno`
  in `ValidatorGroupImpl` and fail validation when candidate parent seqno is behind.
- Committed-proof ingestion hardening: verify `proof.block_id` matches requested
  `block_id` before ingesting downloaded block proofs for MC gap recovery.
- Fix `is_collating` flag leak on early-return paths in `on_generate_slot`
  (explicit parent shard mismatch or parent-too-old), preventing collation pipeline deadlock.
- Max-base merge for `available_base`: align ordering/merge semantics with
  C++ `pool.cpp::add_available_base()` while preventing forward-progress
  regression from out-of-order notarizations and skip propagation.
- Diagnostic dump no longer lists self (local validator) in the inactive nodes summary. `get_last_activity()` in receiver now reports self as always-active (consistent with `calculate_active_weight()`), and `debug_dump()` skips self index in the compact inactive list.
- **Restart gremlin test enabled**: `test_simplex_consensus_restart_gremlin` now passes — `first_non_finalized_slot` correctly advances on skip in all modes.
- DB is now preserved on session stop (previously destroyed prematurely).
- Overlay is registered before bootstrap load completes (prevents missed messages during startup).

### Removed
- `requestCandidate2` / `candidateAndCert2` TL types and all v2 request paths.
  `ENABLE_REQUEST_CANDIDATE_V2` constant removed. `want_final` param removed from
  `request_candidate()`. All FinalCert recovery now uses committed-block proof download.

### Planned (resolved in 0.6.0)
- ~~FinalCert proactive rebroadcast~~ → resolved
- ~~MC fork prevention in validator~~ → resolved
- ~~Adaptive skip timeout increase~~ → resolved
- ~~Precollation parent tracking~~ → superseded by notarized-parent collation mode
- ~~Twostep broadcast via RLDP2~~ → resolved (two-step broadcast TL alignment)
- ~~C++ interoperability testing with testnet~~ → resolved (mixed 5+5 Rust/C++ validation)

---

## [0.4.0] - 2026-02-01

Major release focused on **C++ interoperability** (signatures/certificates/networking), **restart resilience**, and production-grade diagnostics/tests.

**Baseline**: 0.3.0 release commit `b3290888f8b2739d46ca3d914ce9fe91858fa53b`.

**Milestones**
- Single-host Rust network reaches stable consensus under simplex.
- DB-backed bootstrap + restart recovery implemented (vote replay + recommit).
- Certificates can be received/sent and replayed during standstill (C++ catch-up mechanism).

### Added

#### Signature variants (simplex vs ordinary)
- **`BlockSignaturesSimplex`** (`ton_block`): simplex-specific signing context (session_id, slot, candidate_hash_data_bytes, is_final)
- **`BlockSignaturesVariant`** (`ton_block`): unifies Ordinary (catchain) and Simplex signature formats
- TL-B tag `0x12` for simplex signatures (vs `0x11` for ordinary)
- Variant-aware signature verification in `TopBlockDescrStuff::validate_internal()`
- Block JSON support (`ton_block_json`): simplex signatures roundtrip in proofs with `signature_type`
- Consensus callback chain: `on_block_committed` accepts `BlockSignaturesVariant`

#### Certificate messaging + interop
- `candidateAndCert.notar` uses `voteSignatureSet` encoding (C++ compatible)
- parse `consensus.simplex.certificate` messages on vote channel (C++ sends these for catch-up)
- send certificate messages + standstill certificate replay (rebroadcast cached certs on standstill)

#### Persistence + restart
- **DB persistence** (`node/simplex/src/database.rs`): votes, candidate infos, pool state, notar certs, finalized records
- **Startup recovery** (`node/simplex/src/startup_recovery.rs`): bootstrap load, vote replay, receiver cache restore, recommit to ValidatorGroup, last-finalized notification

#### Repair / recovery
- **TEMP and Rust-only**: `requestCandidate2` / `candidateAndCert2` to recover FinalCert signatures (`ENABLE_REQUEST_CANDIDATE_V2`); will be removed in future releases, is needed due to current validator group design limitations
- Masterchain catch-up hardening: FinalCert gating + gap recovery (commit missing MC blocks in order when FinalCert becomes available)

#### Testing & diagnostics
- Manual clock support for deterministic SessionProcessor tests (`SessionDescription::set_time` / `advance_time`)
- Gremlin/partition test harness hardening and improved debug dumps

### Changed

- **Round handling**: removed `current_round`-style tracking and related gauges; use roundless/slot-derived behavior for ValidatorGroup compatibility
- Receiver hot-path logging compacted; notar cert caches store serialized bytes (no TL-object caching)
- Standstill replay behavior hardened and aligned with the C++ approach

### Fixed

- Block acceptance signature verification: simplex signatures no longer fail in `accept_block`
- Vote signatures: session-scoped wrapper (`consensus.dataToSign(session_id, vote_bytes)`) matches C++ `pool.cpp`
- `SimplexState::process_timeouts()` loop robustness
- seqno mismatch panic after partition recovery
- Gremlin stability: do not drop notar bytes received via repair when candidate body is already known

### Removed

- `is_block_finalized()` method from SessionProcessor (unused)
- `get_candidate_id()` method from SimplexState (unused)

### Documentation

- Updated `README.md`, implementation plan, and interop discrepancy inventory

---

## [0.3.0] - 2026-01-14

Major release focusing on candidate resolution, certificate system, and operational stability.

### Added

#### Certificate System
- **`certificate.rs`** module: Generic `Certificate<T>` struct for vote aggregation
- `VoteSignature` struct with `validator_idx: ValidatorIndex` and `signature: Vec<u8>`
- Type aliases: `NotarCert`, `SkipCert`, `FinalCert` for typed certificates
- `VerifiableVote` trait: Abstracts vote-specific verification for generic certificate parsing
- TL serialization via `from_tl()`, `to_tl()`, `to_tl_vote_signature_set()`
- Signature verification for all certificate types
- 14 unit tests in `test_certificate.rs`

#### Candidate Resolver
- **`CandidateResolverCache`** in `receiver.rs`: Single-threaded cache for candidates and certificates
- **Query handler**: Processes `consensus.simplex.requestCandidate` queries
- **Response builder**: Creates `consensus.simplex.candidateAndCert` responses
- Cache uses `HashMap<(SlotIndex, UInt256), Vec<u8>>` with type-safe keys
- Cleanup with `MAX_HISTORY_SLOTS=1024` for bounded memory
- 8 unit tests in `test_candidate_resolver.rs` (via `#[path]` module)

#### Query Support / Block Repair
- **Delayed actions in ReceiverImpl**: `ReceiverDelayedAction` for scheduled tasks
- **Candidate request state**: `CandidateRequestState` with retry tracking
- **Request/response flow**: `request_candidate_impl()`, `handle_candidate_response()`
- **Retry mechanism**: Random peer selection, up to 5 retries per candidate
- **SessionProcessor request tracking**: `requested_candidates` HashSet with 1-second delay before request
- `schedule_request_candidate()` with delayed action for broadcast tolerance
- Generic certificate verification using `VerifiableVote` trait

#### Recursive Candidate Resolution
- **Parent chain tracking**: Validates parent availability before candidate validation
- `PendingParentResolution` struct for queued candidates awaiting parents
- `compute_is_fully_resolved()`: Check if full parent chain is available
- `find_first_missing_parent()`: Walk chain to find first gap
- `update_resolution_cache_chain()`: Recursively update resolution status
- `try_resolve_waiting_candidates()`: Process waiting candidates when parent arrives
- Constants: `MAX_CHAIN_DEPTH=10000`, `MAX_PARENT_WAIT_TIME=10min`

#### Parent Validation Mode (C++ Compatibility)
- **`require_finalized_parent`** flag in `SimplexStateOptions` (default: `false` for C++ compatibility)
- When `false`: notarized blocks can serve as parents (C++ behavior)
- When `true`: only finalized blocks can be parents (strict mode for testing)
- `is_parent_valid()` helper for checking parent availability
- `SimplexStateOptions::strict_sequential()` factory for testing with strict mode
- 8 new unit tests for parent validation modes and deadlock scenarios

#### SessionProcessor Round Tracking
- **`current_round`** field: Tracks sequential commit counter (independent of slots)
- Used in callbacks: `on_generate_slot`, `on_candidate`, `on_block_committed`
- Increments by 1 for every committed block (including empty blocks)
- Removed `current_slot` field from SessionProcessor (use FSM's `get_current_slot()`)
- `round_debug_at`: Debug dump only when no commits for `ROUND_DEBUG_PERIOD` (15s)

#### Batch Finalization
- **`collect_parent_chain()`**: Collects all parent blocks for batch commit
- **`commit_single_block()`**: Commits individual block with seqno validation
- **`BlockToCommit`** struct: Holds block info during batch commit
- **`finalized_blocks`** HashSet: Tracks committed blocks to avoid double-commit
- Seqno validation: Asserts `seqno == last_committed_seqno + 1` (hard invariant)
- 3 new batch finalization tests

#### Standstill Resolution (Aligned with C++)
- `reschedule_standstill()`: Called only on finalization (not skip)
- `set_standstill_slots(begin, end)`: Sets vote re-broadcast range
- Votes filtered to `[first_non_finalized_slot, current_window_end)`
- Initial range `[0, 1_000_000)` before first finalization

#### Network Metrics
- `simplex_receiver_in_messages_count`: Incoming vote messages
- `simplex_receiver_out_messages_count`: Outgoing vote messages  
- `simplex_receiver_in_broadcasts_count`: Incoming block broadcasts
- `simplex_receiver_out_broadcasts_count`: Outgoing block broadcasts
- `simplex_receiver_in_queries_count`: Incoming candidate queries

#### SessionProcessor Metrics
- `simplex_skipped_slots`: Counter for slot skip events
- `simplex_batch_commits`: Counter for batch commit operations
- `simplex_batch_commit_size`: Histogram for batch commit sizes (blocks per batch)
- `simplex_current_round`: Gauge for current round (sequential commit counter)

#### Test Infrastructure
- **Test serialization mutex**: `SIMPLEX_TEST_MUTEX` prevents parallel consensus test execution
- **ADNL startup delay**: 3-second stabilization delay for ADNL overlay tests
- **Test receiver candidate resolver**: Integration test for P2P candidate query flow
- Test count increased from 163 to 168 tests

### Changed

#### API Changes
- **`on_block_broadcast` → `on_candidate_received`**: Unified callback with optional `notar_cert`
- `ReceiverListener::on_candidate_received(source_idx, candidate, notar_cert: Option<Vec<u8>>)`
- **Removed `cache_candidate`** from `Receiver` trait: Caching now internal to ReceiverImpl
- **`SimplexState::cleanup_slots`** called externally: SessionProcessor controls cleanup timing (respects first_non_finalized_slot)
- **`on_block_skipped` deprecated**: Skip events handled internally, callback not invoked in production
- **Callbacks use `current_round`**: `on_generate_slot`, `on_candidate`, `on_block_committed` use sequential round counter

#### Receiver Improvements
- `send_block_broadcast_impl()` now caches candidate internally after broadcast
- `process_block_broadcast()` caches on receive before validation
- `select_peer_for_candidate_request()`: Random peer selection (removed `start_idx` parameter)
- `calculate_active_weight()`: Always includes local validator weight
- Explicit overlay shutdown in `ReceiverWrapper::stop()` and `Drop` impl

#### SessionProcessor Improvements
- `generated_block()`: Synchronous `on_candidate_received` call (fixes race condition)
- `generated_block()`: Reset `pending_generate` on error (allows retry)
- Seqno mismatch errors downgraded to warnings (timing issue, not critical)
- Debug dump formatting: Removed duplicate prefixes (`v`, `w`, `s`)
- Local validator shown as "self" instead of "inactive" in dump

#### SimplexState Improvements
- `set_notarize_certificate()`: Updates vote accounting when receiving external certificate
- Certificate creation uses actual signatures from vote tracking
- `NotarCert` missing at finalization: Warning (not panic) with empty signatures
- Per-block notarization certificate caching (not per-slot)

### Fixed

#### Critical Fixes
- **Seqno mismatch race condition**: Locally generated blocks now processed synchronously
- **ADNL test hang at exit**: Explicit overlay shutdown added to receiver cleanup
- **Slot gap panic**: FSM now enforces sequential event emission internally
- **NotarCert invariant violation**: Per-block certificate caching prevents missed certs

#### Minor Fixes
- `send_block_broadcast_impl()`: Cache candidate for query responses
- Certificate weight check: Verify ≥2/3 threshold before accepting external certs
- Timeout trace logging: Log when request was already fulfilled

### Documentation
- Candidate resolver diagrams added
- Updated `README.md` with new test counts and component descriptions
- Inline documentation updated for `receiver.rs`, `session_processor.rs`
- `VerifiableVote` trait documented with usage examples

### Known Issues
- Twostep broadcast not implemented (candidate resolver may make it unnecessary)
- Precollation parent tracking needed for cross-window scenarios (precollation currently disabled)
- DB persistence not implemented (required for restart support)

---

## [0.2.0] - 2026-01-07

### Added

#### Empty Block Support (Finalization Recovery)
- **`consensus.empty` TL variant**: Full implementation for empty block handling
- `compute_candidate_id_hash_empty()` for proper hash computation with `candidateHashDataEmpty`
- `create_empty_block_desc()` for TL object creation
- Receiver handles both `Consensus_Block` and `Consensus_Empty` variants
- `should_generate_empty_block()` logic matching C++ implementation
- `set_mc_finalized_seqno()` and `notify_mc_finalized()` for MC finalization tracking
- `empty_block_mc_lag_threshold` option for shardchain sessions

#### C++ Protocol Compatibility
- **`enable_fallback_protocol` option**: Controls fallback vote behavior
  - Default: `false` (C++ compatible, 3 vote types: Notarize, Finalize, Skip)
  - When `true`: Full Alpenglow White Paper algorithm with fallback votes
- **`allow_skip_after_notarize` option**: C++ allows skip after notarize (default: `true`)
- **TL schema synced**: Wire protocol uses only 3 vote types in C++ mode
- Fallback votes (`NotarizeFallback`, `SkipFallback`) filtered at broadcast layer

#### Memory Management
- **Dedup cleanup**: Vote deduplication entries cleaned in `Receiver::cleanup_slot()`
- **Validated candidates cleanup**: `cleanup_old_candidates()` called from `reset_slot_state()`
- `MAX_PAST_SLOT_CANDIDATES = 64` buffer for received candidates retention

#### Block Handling Improvements
- **Slot vs SeqNo fix**: Correct seqno tracking throughout (uses BlockIdExt, not slot)
- `ValidatorBlockId` → `BlockIdExt` migration (cherry-picked from master)
- `SessionStats` tracking in `on_block_committed` callback

### Changed

#### Dependency Restructuring
- **Removed dependencies** on `catchain` and `validator_session` crates
- **Now imports ONLY from `consensus-common`** for all shared types
- All overlay interfaces now use `Consensus*` naming (e.g., `ConsensusOverlay`, `ConsensusOverlayManager`)
- Compression utilities imported from `consensus_common::compression`
- Serialization macros imported from `consensus_common`

#### Type Imports Updated
- `SessionListener` → from `consensus_common`
- `Session` (base trait) → from `consensus_common` (aliased as `ConsensusSession`)
- `SessionNode`, `SessionStats` → from `consensus_common`
- `BlockSourceInfo`, `BlockCandidatePriority` → from `consensus_common`
- `ValidatorBlockCandidate`, `ValidatorBlockCandidatePtr` → from `consensus_common`
- `AsyncRequest`, `AsyncRequestPtr` → from `consensus_common`
- `compress_candidate_data`, `decompress_candidate_data` → from `consensus_common::compression`

#### API Changes
- `SimplexSession` trait now extends `consensus_common::Session` (previously standalone)
- Callback result types now use `crate::Result<T>` (anyhow::Error) instead of `Result<T, String>`
- `on_candidate()` callback now passes `BlockPayloadPtr` directly (not raw bytes)

### Documentation
- Updated README.md architecture diagrams to reference `consensus-common`
- Updated dependencies table in README
- Removed local file path references from References section

---

## [0.1.0] - 2026-01-02

### Added

#### Core Components
- **Session Actor** (`session.rs`)
  - Multi-threaded session with SXMAIN, SXCB, SXRCV threads
  - Task queues for cross-thread communication
  - MetricsDumper integration for periodic metrics logging
  - `stop_async()` for non-blocking session stop

- **SessionProcessor** (`session_processor.rs`)
  - Single-threaded consensus core integrating SimplexState
  - Collation flow with precollation pipeline
  - Validation flow with retry mechanism
  - Finalization flow with signature collection
  - Debug dump with compact/full modes
  - Delayed actions for scheduled operations

- **SimplexState FSM** (`simplex_state.rs`)
  - Core consensus state machine
  - Vote accounting: Notarize, Finalize, Skip, NotarizeFallback, SkipFallback
  - Threshold detection: 2/3 (notarization/finalization), 1/3 (fallback triggers)
  - Leader window management with `available_bases`
  - Parent propagation via `on_parent_ready()`
  - Misbehavior detection for conflicting votes
  - `debug_dump(full_dump: bool)` for compact/full state output

- **Receiver** (`receiver.rs`)
  - Network layer with overlay integration
  - Vote and block broadcast handling
  - Signature verification for incoming messages
  - Deduplication of incoming votes
  - Activity tracking per validator
  - Send order randomization (every 10s)

- **Block Types** (`block.rs`)
  - `RawCandidateId` with hash computation
  - `CandidateId` with BlockIdExt resolution
  - `RawCandidate` with LZ4 compression support
  - `Candidate` with parent resolution
  - `CandidateParentInfo` for FSM operations

- **Utilities** (`utils.rs`)
  - Session-scoped signature creation and verification
  - Candidate hash computation
  - Vote TL serialization/deserialization
  - Block info extraction from compressed candidates

- **Session Description** (`session_description.rs`)
  - Validator configuration and indexing
  - Threshold calculations (2/3, 1/3)
  - Leader selection (round-robin by window)
  - Slot/window helper methods

#### Test Infrastructure
- **Unit Tests** (76 total)
  - 45 FSM tests in `simplex_state.rs`
  - 13 crypto tests in `tests/test_crypto.rs`
  - 10 block tests in `tests/test_block.rs`

- **Integration Tests**
  - `test_collation.rs` - Single-node collation test
  - `test_consensus.rs` - Multi-validator consensus (2 tests)
  - `test_receiver.rs` - Receiver unit tests
  - `test_validation.rs` - Two-node validation test

- **Test Features**
  - Error log detection (tests fail on ERROR logs)
  - Commit latency statistics (min, max, median, avg, sigma, 95% CI)
  - Configurable test parameters via `TestConfig`

#### Metrics
- Counters: `check_all`, `process_events`, `precollation_*`
- Histograms: `slot_duration`, `validation_latency`, `collation_latency`
- ResultStatusCounters: `validates`, `collates`, `commits`
- Gauges: `active_weight`, `total_weight`, `threshold_66`

#### Documentation
- `README.md` with architecture diagrams
- Inline documentation with C++ references

### Known Issues (Fixed in 0.2.0)
- ~~TL schema is outdated~~ → Fixed: TL schema synced with C++ (3 vote types on wire)
- ~~Empty blocks not supported~~ → Fixed: Full `consensus.empty` TL variant support
- ~~Memory leaks in dedup and candidates maps~~ → Fixed: Cleanup routines added
- No cross-implementation testing with C++ nodes yet (still pending)

---

## Version History

| Version | Date | Tag | Description |
|---------|------|-----|-------------|
| 0.7.1 | 2026-06-09 | `simplex-0.7.1` | Async DB persistence off the SXMAIN thread, restart-recovery hardening (skip/final-cert replay + base repair), non-fatal invariant handling (skipscan / finalized dedup / far-future FinalCert), requestCandidate repair validation, shard collation timing parity, opt-in block-sync overlay (observers) |
| 0.7.0 | 2026-04-21 | `simplex-0.7.0` | State resolver for ghost-parent collation, cert order + DB-wait-order durability, bootstrap-deadlock fixes, bad-signature peer-ban DoS hardening, per-session Prometheus republishing |
| 0.6.0 | 2026-04-08 | `simplex-0.6.0` | Finalized-driven delivery, C++ parity overhaul, legacy mode removal, stall diagnostics |
| 0.5.0 | 2026-03-20 | `simplex-0.5.0` | Committed-block proof recovery, restart gremlin fix, requestCandidate2 removal, parity docs update |
| 0.4.0 | 2026-02-01 | `simplex-0.4.0` | Block signature types, C++ compatibility, restart resilience |
| 0.3.0 | 2026-01-14 | `simplex-0.3.0` | Candidate resolver, certificates, operational stability |
| 0.2.0 | 2026-01-07 | `simplex-0.2.0` | consensus-common integration, dependency restructuring |
| 0.1.0 | 2026-01-02 | `simplex-0.1.0` | Initial implementation with core consensus |

---



