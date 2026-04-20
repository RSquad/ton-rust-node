# Changelog

All notable changes to the Simplex Consensus Protocol implementation will be documented in this file.

## [Unreleased]

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
| 0.6.0 | 2026-04-08 | `simplex-0.6.0` | Finalized-driven delivery, C++ parity overhaul, legacy mode removal, stall diagnostics |
| 0.5.0 | 2026-03-20 | `simplex-0.5.0` | Committed-block proof recovery, restart gremlin fix, requestCandidate2 removal, parity docs update |
| 0.4.0 | 2026-02-01 | `simplex-0.4.0` | Block signature types, C++ compatibility, restart resilience |
| 0.3.0 | 2026-01-14 | `simplex-0.3.0` | Candidate resolver, certificates, operational stability |
| 0.2.0 | 2026-01-07 | `simplex-0.2.0` | consensus-common integration, dependency restructuring |
| 0.1.0 | 2026-01-02 | `simplex-0.1.0` | Initial implementation with core consensus |

---



