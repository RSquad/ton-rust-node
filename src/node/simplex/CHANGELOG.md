# Changelog

All notable changes to the Simplex Consensus Protocol implementation will be documented in this file.

## [Unreleased]

### Added
- **GET-COMMITTED-1**: Download committed block via full-node proof for MC gap recovery.
  Replaces Rust-only `requestCandidate2` with C++-compatible mechanism.
  `SessionListener::get_committed_candidate` trait method, `CommittedBlockProof` type,
  `ValidatorGroup::on_get_committed_candidate` implementation using `download_block_proof()`.
- `test_simplex_consensus_finalcert_recovery`: FinalCert-recovery gremlin test with per-node
  lossy overlay targeting (7 MC nodes, node 0 gets 40% broadcast + 30% message/query loss).
- `lossy_overlay_node_indices` field in `LossyOverlayOpts` for per-node loss targeting.
- **NODE-20 (OBS-1)**: C++-parity standstill slot-grid dump (`standstill_slot_grid_dump()`
  on `SimplexState`). Mirrors C++ `pool.cpp::alarm()` per-validator markers (F/I/N/S/.)
  and cert flags (notar/skip/final). Wired into `debug_dump()` on stall detection.
- **NODE-19 (HEALTH-1)**: Receiver-side anomaly checks with configurable thresholds.
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
  (C++ `block-validator.cpp` commit `9aac62b8` parity). Track `last_accepted_mc_seqno`
  in `ValidatorGroupImpl` and fail validation when candidate parent seqno is behind.
- Committed-proof ingestion hardening: verify `proof.block_id` matches requested
  `block_id` before ingesting downloaded block proofs for MC gap recovery.
- Fix `is_collating` flag leak on early-return paths in `on_generate_slot`
  (explicit parent shard mismatch or parent-too-old), preventing collation pipeline deadlock.
- Max-base merge for `available_base`: align ordering/merge semantics with
  C++ `pool.cpp::add_available_base()` while preventing forward-progress
  regression from out-of-order notarizations and skip propagation.
- **TN-754**: Diagnostic dump no longer lists self (local validator) in the inactive nodes summary. `get_last_activity()` in receiver now reports self as always-active (consistent with `calculate_active_weight()`), and `debug_dump()` skips self index in the compact inactive list.
- **Restart gremlin test enabled**: `test_simplex_consensus_restart_gremlin` now passes — `first_non_finalized_slot` correctly advances on skip in all modes.
- DB is now preserved on session stop (previously destroyed prematurely).
- Overlay is registered before bootstrap load completes (prevents missed messages during startup).

### Removed
- `requestCandidate2` / `candidateAndCert2` TL types and all v2 request paths.
  `ENABLE_REQUEST_CANDIDATE_V2` constant removed. `want_final` param removed from
  `request_candidate()`. All FinalCert recovery now uses GET-COMMITTED-1.

### Planned
- FinalCert proactive rebroadcast (C++ `cfd8850c` parity)
- MC fork prevention in validator (C++ `9aac62b8` parity)
- Adaptive skip timeout increase (C++ `3c0cae03` parity)
- Precollation parent tracking (lock parent at start of collation)
- Twostep broadcast via RLDP2
- C++ interoperability testing with testnet

---

## [0.5.0] - 2026-02-01

**Baseline**: 0.4.0 release.

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
| 0.5.0 | 2026-02-28 | `simplex-0.5.0` | GET-COMMITTED-1, restart gremlin fix, requestCandidate2 removal |
| 0.4.0 | 2026-02-01 | `simplex-0.4.0` | Block signature types, C++ compatibility, restart resilience |
| 0.3.0 | 2026-01-14 | `simplex-0.3.0` | Candidate resolver, certificates, operational stability |
| 0.2.0 | 2026-01-07 | `simplex-0.2.0` | consensus-common integration, dependency restructuring |
| 0.1.0 | 2026-01-02 | `simplex-0.1.0` | Initial implementation with core consensus |

---

[Unreleased]: https://github.com/RSquad/ton-node/compare/simplex-0.5.0...HEAD
[0.5.0]: https://github.com/RSquad/ton-node/compare/simplex-0.4.0...simplex-0.5.0
[0.4.0]: https://github.com/RSquad/ton-node/compare/simplex-0.3.0...simplex-0.4.0
[0.3.0]: https://github.com/RSquad/ton-node/compare/simplex-0.2.0...simplex-0.3.0
[0.2.0]: https://github.com/RSquad/ton-node/compare/simplex-0.1.0...simplex-0.2.0
[0.1.0]: https://github.com/RSquad/ton-node/releases/tag/simplex-0.1.0

