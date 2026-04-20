# Simplex Consensus Protocol

**Version**: 0.6.0 (April 8, 2026) | [Changelog](CHANGELOG.md)

Rust implementation of the Simplex consensus protocol for TON blockchain.

> **C++ Reference**: Primary tracking is [ton-blockchain/ton](https://github.com/ton-blockchain/ton) (`testnet/validator/consensus/simplex`).
>
> **Protocol Spec**: [ton-blockchain/simplex-docs](https://github.com/ton-blockchain/simplex-docs) (`Simplex.md`).

> **Current semantics (Apr 2026, v0.6.0):**
> - Simplex is finalized-driven.
> - Finalized blocks are delivered through `on_block_finalized()` and may arrive out of order.
> - `on_block_committed()` remains part of the shared listener interface for legacy sequential acceptance, but Simplex must not use it.
> - Missing-body handling uses `finalized_pending_body`: a finalized block can be known before its body arrives locally.
> - Historical Rust-only fallback/strict-parent mode has been removed; only the C++-compatible three-vote behavior is supported.
> - Session creation is separated from start: `create_session()` + `start(initial_block_seqno)`.
> - Restart recovery is state-restoration only — no historical replay callbacks.

## Overview

Simplex is a consensus protocol with TON-specific implementation choices:

- **Conservative path only** (no fast finality/optimistic path)
- **Fault tolerance**: <1/3 Byzantine nodes
- **Certificate threshold**: 2/3 stake weight
- **No erasure coding**: Simple broadcast instead of Rotor shreds

### Key Design Decisions

1. **Conservative consensus path**: Focus on reliability over speed
2. **Ed25519 signatures**: Individual signatures, no BLS aggregation
3. **Actor model**: Separate threads for consensus, callbacks, and network
4. **Task queues**: Cross-thread communication via closures

### Protocol Mapping (Simplex.md -> C++ -> Rust)

| Simplex.md Concept | C++ Touchpoint | Rust Touchpoint |
|---|---|---|
| `tryNotar` | `consensus.cpp::try_notarize` | `simplex_state.rs::try_notar` |
| `tryFinal` | finalize gating in `consensus.cpp` | `simplex_state.rs::try_final` |
| `trySkipWindow` / timeout alarm | `consensus.cpp::alarm` | `simplex_state.rs::process_timeouts`, `simplex_state.rs::try_skip_window` |
| Certificate ingestion | `pool.cpp::handle_foreign_certificate` | `simplex_state.rs::set_notarize_certificate`, `set_finalize_certificate`, `set_skip_certificate` |
| Progress cursor / leader-window publish | `pool.cpp::maybe_publish_new_leader_windows` | `simplex_state.rs::advance_progress_cursor`, `advance_leader_window_on_progress_cursor` |

### Relationship to Other Components

```
validator-manager (higher level)
        │
        │ SessionListener callbacks
        ▼
    simplex ◄── this crate
        │
        │ imports from
        ▼
consensus-common (shared types, overlay interfaces, compression)
        │
        │ implements
        ▼
overlay / ADNL (lower level, network)
```

## Rust vs C++ reference: known differences

This crate targets wire-compatibility with the upstream **C++ Simplex** implementation in [ton-blockchain/ton](https://github.com/ton-blockchain/ton) (`testnet/validator/consensus/simplex`).

### Protocol parity gaps (from C++ upstream)

- Certificate persistence ordering — C++ persists certs before state transitions. **HIGH**
- Deterministic vote replay ordering — wait-for-store semantics. **MEDIUM**
- Anti-spam / DOS hardening — overlay-level antispam. **MEDIUM**
- External-aware collation pipeline — callback-driven external wait loop. **MEDIUM**
- Ghost-parent MC collation deadlock — notarized-but-unfinalized parent state unavailability. **HIGH**
- Speculative state resolver — in-process state computation for unfinalized parents. **MEDIUM**

### Implementation parity gaps

- C++ has `ImprovedStructureLZ4WithState` (BOC compression algo 2) — Rust only supports algos 0 and 1.
- C++ has `StoreCellHint` for DB commit optimization during MerkleUpdate apply — Rust lacks equivalent.
- C++ overlay manager can buffer messages for unknown overlays (disabled by default) — Rust lacks equivalent.

### Resolved (for reference)

- Finalized-driven delivery: Simplex delivers through `on_block_finalized()`, matching C++ out-of-order finalized model.
- Parent gating aligned with C++ flow: `is_wait_for_parent_ready()` mirrors `pool.cpp::maybe_resolve_request()`.
- Candidate chaining within leader windows matches C++ `pool.cpp`.
- Empty-candidate FSM-tip validation: reject unless referenced block matches parent normal tip.
- Available-base propagation on leader window advancement.
- Two-step broadcast TL schema alignment: all 10 nodes producing in mixed 5x5 test.
- Timing wake discipline and block-rate cap parity: `min_block_interval_ms` wired.
- Candidate signature now signs bare `consensus.candidateId` directly, matching C++ testnet. Regression test: `test_candidate_id_to_sign_is_bare_candidate_id`.
- MC stale-head rejection implemented in `validator_group.rs` (`should_reject_stale_mc_candidate`), matching C++ `block-validator.cpp`.
- Adaptive first-block timeout backoff after skip implemented in `simplex_state.rs` (`apply_adaptive_timeout_backoff`), matching C++ `consensus.cpp`.
- Twostep FEC broadcast implemented in `consensus-common/adnl_overlay.rs` (`BroadcastTwostepSimple`), with C++-compatible signing.
- QUIC transport supported via `SessionOptions::use_quic` and `OverlayTransportType::SimplexQuic`. Tested in `test_adnl_overlay_quic_delivery`.
- Overlay ID computation (node ordering, short ID)
- `candidateAndCert.notar` encoding (voteSignatureSet)
- Handle incoming `consensus.simplex.certificate` on vote channel
- `requestCandidate2` removed
- `get_committed_candidate` / `CommittedBlockProof*` removed; current Simplex relies on finalized delivery plus deferred body materialization
- Shard `before_split` empty block rule
- Restart support (DB persistence + startup recovery)
- Certificate rebroadcast on restart
- FinalCert proactive rebroadcast (Rust local-creation broadcast behavior)
- Base selection uses progress cursor (`first_non_progressed_slot`) for leader-window advancement
- Committed-parent validation gate (superseded by finalized-driven delivery model)


## Architecture

### Threading Model

```
┌────────────────────────────────────────────────────────────────────────────────┐
│ Simplex Session                                                                │
│                                                                                │
│  ┌────────────────────────────────────┐   ┌──────────────────────────────────┐ │
│  │ Main Thread (SXMAIN:{session_id})  │   │ Callback Thread (SXCB:...)       │ │
│  │                                    │   │ (if use_callback_thread=true)    │ │
│  │  ┌──────────────────────────────┐  │   │                                  │ │
│  │  │ SessionProcessor             │  │   │  - pull callback queue           │ │
│  │  │  - consensus FSM             │  │   │  - invoke SessionListener:       │ │
│  │  │  - slot state management     │  │   │    - on_candidate                │ │
│  │  │  - vote tracking             │  │   │    - on_block_finalized          │ │
│  │  └──────────────────────────────┘  │   │    - on_generate_slot            │ │
│  │                                    │   └──────────────────────────────────┘ │
│  │  - pull main task queue            │                                        │
│  │  - check_all() on awake            │                                        │
│  │  - metrics dump (30s)              │                                        │
│  └────────────────────────────────────┘                                        │
│                         ▲                                                      │
│                         │ ReceiverListener callbacks                           │
│                         │                                                      │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ Receiver Thread (SXRCV:{session_id})                                    │   │
│  │                                                                         │   │
│  │  - deserialize incoming TL messages                                     │   │
│  │  - verify signatures, deduplicate                                       │   │
│  │  - post to main queue via ReceiverListener                              │   │
│  │  - serialize and send outgoing votes/broadcasts                         │   │
│  │  - metrics dump (30s), shuffle send order (10s)                         │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
│                         │                                                      │
│                         │ ConsensusOverlay                                     │
│                         ▼                                                      │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ ConsensusOverlayManager (from consensus-common)                         │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────────────────┘
```

### Data Flow

```
                    Incoming                              Outgoing
                        │                                     ▲
                        ▼                                     │
┌─────────────────────────────────────────────────────────────────────────────┐
│ Receiver                                                                    │
│  1. Deserialize TL (Vote or BlockBroadcast)                                 │
│  2. Validate signature (verify with source public key)                      │
│  3. Deduplicate (per-slot HashMap keyed by signature hash)                  │
│  4. Post closure to Session main queue                                      │
└─────────────────────────────────────────────────────────────────────────────┘
                        │                                     ▲
                        │ post_closure                        │ send_vote/broadcast
                        ▼                                     │
┌─────────────────────────────────────────────────────────────────────────────┐
│ SessionProcessor                                                            │
│  1. Pull task from main queue                                               │
│  2. Process vote → update slot state, check thresholds                      │
│  3. Emit events (BlockNotarized, BlockFinalized, SlotSkipped, etc.)         │
│  4. May broadcast new vote via Receiver                                     │
└─────────────────────────────────────────────────────────────────────────────┘
                        │
                        │ callback (if use_callback_thread)
                        ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SessionListener (implemented by caller)                                     │
│  - on_candidate: validate block                                             │
│  - on_generate_slot: create new block                                       │
│  - on_block_finalized: finalized-block delivery                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Key Concepts

### Leader Windows

Slots are grouped into **leader windows**. One leader is responsible for all slots in a window:
- Window index: `slot / slots_per_leader_window`
- Leader selection: round-robin by window index
- First slot in window can build on any notarized block
- Subsequent slots must build on previous slot's voted block

### Vote Types

| Vote | On Wire | Purpose |
|------|---------|---------|
| `NotarizeVote` | ✅ | Vote to notarize a block in a slot |
| `FinalizeVote` | ✅ | Vote to finalize after notarization |
| `SkipVote` | ✅ | Skip a slot (timeout or no valid block) |

### Certificates

When 2/3 stake weight is reached for a vote type, a certificate is formed:
- **NotarizationCert**: Block is notarized
- **FinalizationCert**: Block is finalized
- **SkipCert**: Slot is skipped

Certificates are implicit (derived from vote counts), not explicit on-wire objects.

### Empty Blocks (TON-Specific Extension)

Empty blocks are a **finalization recovery** mechanism not in the original protocol paper:

**Purpose**: When consensus gets ahead of finalization (no FinalizeCertificates), empty blocks
let validators re-vote on the previous block to attempt getting a FinalizeCertificate.

**When generated** (`should_generate_empty_block()`):
- **Masterchain**: When `last_finalized_seqno + 1 < new_seqno`
- **Shardchain**: When `last_mc_finalized_seqno + 8 < new_seqno`

**Key invariants**:
- First block in epoch **cannot** be empty (must have actual data)
- Empty block **must** have parent (inherits parent's `BlockIdExt`)
- Empty blocks use `consensus.empty` TL variant (not `consensus.block`)

**Implementation** (in `session_processor.rs`):
- `CollationResult` enum: `Block(candidate)` or `Empty { parent_block_id }`
- `GeneratedBlockDesc`: Common data for both empty and normal blocks
- `create_normal_block_desc()` / `create_empty_block_desc()`: Prepare block data

### Thresholds

| Threshold | Value | Purpose |
|-----------|-------|---------|
| 2/3 (66%) | `(total * 2 + 2) / 3` | Certificate formation |
| 1/3 (33%) | `(total + 2) / 3` | Helper quorum threshold |

### Consensus Loop

Each slot follows this flow:

```
Collate → Broadcast → Validate → Notarize → Vote → Collect → Finalize → Deliver → next slot
```

| Phase | SessionProcessor | SimplexState | Output |
|-------|-----------------|--------------|--------|
| **Collate** | `check_collation()` → `invoke_collation()` | - | Block candidate |
| **Broadcast** | `generated_block()` → `receiver.send_broadcast()` | - | Block to network |
| **Validate** | `on_block_broadcast()` → `notify_candidate()` | - | Validation request |
| **Notarize** | `candidate_decision()` | `on_candidate()` → `try_notar()` | `BroadcastVote(Notar)` |
| **Vote** | `broadcast_vote()` | - | Vote to network |
| **Collect** | `on_vote()` | `on_vote()` → thresholds | Threshold events |
| **Finalize** | - | `try_final()` | `BroadcastVote(Final)` |
| **Deliver** | `handle_block_finalized()` | `BlockFinalized` event | `on_block_finalized()` |

## Package Structure

```
node/simplex/
├── Cargo.toml                 # Package manifest
├── README.md                  # This file
├── CHANGELOG.md               # Release notes (this crate)
├── src/
│   ├── lib.rs                 # Public API: Session, SimplexSession, SessionOptions, SessionFactory
│   ├── block.rs               # Block candidate types: RawCandidateId, Candidate, etc.
│   ├── certificate.rs         # Certificate types: VoteSignature, Certificate<T> (crate-private)
│   ├── database.rs            # DB persistence for restart recovery (crate-private)
│   ├── simplex_state.rs       # Core consensus FSM with event-based output
│   ├── session.rs             # Session actor (multi-threaded wrapper, task queues)
│   ├── session_processor.rs   # Integrates SimplexState with network (crate-private)
│   ├── session_description.rs # Session constants and validators info (crate-private)
│   ├── startup_recovery.rs    # Startup recovery / restart replay (crate-private)
│   ├── task_queue.rs          # Task queue traits and types (crate-private)
│   ├── receiver.rs            # Network overlay management (crate-private)
│   ├── utils.rs               # Signature verification, hash computation, thresholds
│   ├── misbehavior.rs         # Misbehavior detection: MisbehaviorProof, MisbehaviorReport, RawVoteData
│   └── tests/                 # Internal unit tests (crate-private)
│       ├── mod.rs
│       ├── test_block.rs
│       ├── test_candidate_resolver.rs
│       ├── test_certificate.rs
│       ├── test_crypto.rs
│       ├── test_database.rs
│       ├── test_misbehavior.rs
│       ├── test_receiver.rs
│       ├── test_restart.rs
│       ├── test_session_description.rs
│       ├── test_session_processor.rs
│       ├── test_simplex_state.rs
│       └── test_slot_bounds.rs
└── tests/
    ├── test_collation.rs      # Single-node collation integration test
    ├── test_consensus.rs      # Multi-instance consensus integration tests
    ├── test_restart.rs        # Restart integration tests (public API only)
    └── test_validation.rs     # Two-node validation integration test
```

## Components

### Public API (`lib.rs`)

Entry point for integration. See `lib.rs` documentation for detailed API reference.

| Type | Purpose |
|------|---------|
| `SessionFactory` | Factory for creating sessions and overlay managers |
| `SessionOptions` | Configuration options for sessions |
| `ConsensusSession` | Base session interface (trait, from consensus-common) |
| `SimplexSession` | Simplex-specific session operations (extends `ConsensusSession`) |
| `SessionListener` | Callback trait (from consensus-common) |
| `SessionStats` | Session health metrics passed alongside validator callbacks |
| `Receiver` | Network sender interface (trait) |
| `ReceiverListener` | Network receiver callbacks (trait) |

**SimplexSession Trait** (for MC finalization notification):

```rust
pub trait SimplexSession: ConsensusSession {
    /// Notify session about accepted MC top (for shard empty block decisions)
    fn notify_mc_finalized(&self, applied_top: BlockIdExt);
}
```

This separate trait allows simplex-specific operations without modifying the shared `ConsensusSession` trait
from validator-session. For shard chains, the higher layer (ValidatorManager) should call
`notify_mc_finalized()` with the accepted MC top `BlockIdExt` when masterchain blocks are finalized to enable empty block generation.

### Session (`session.rs`)

Multi-threaded wrapper managing:
- Main loop thread (`SXMAIN:*`) for consensus processing
- Optional callback thread (`SXCB:*`) for listener callbacks
- Task queues for cross-thread communication
- Activity node for liveness tracking
- Metrics and profiling (dump every 30s)
- Receiver creation and lifecycle

### SessionProcessor (`session_processor.rs`)

Single-threaded consensus algorithm (crate-private):
- Integrates SimplexState FSM with network layer
- Processes receiver callbacks (votes, candidates)
- Pulls SimplexState events and dispatches to network/listener
- Contains ASCII flow diagrams for: Collation, Precollation, Validation, Finalization

**Current implementation:**
- ✅ SimplexState FSM integration (`simplex_state` field)
- ✅ Delayed actions infrastructure (`post_delayed_action()`, `process_delayed_actions()`)
- ✅ Event processing loop (`process_simplex_events()`)
- ✅ Metrics infrastructure (`MetricsHandle`, counters, histograms, gauges)
- ✅ Vote handling (`on_vote()`, `broadcast_vote()`) - TL serialization done
- ✅ Collation flow (`check_collation()`, `invoke_collation()`, `generated_block()`)
- ✅ Precollation pipeline (`precollate_block()`, `remove_precollated_block()`)
- ✅ Block finalization (`handle_block_finalized()`) - signature collection done
- ✅ Validation flow (`on_block_broadcast()`, `check_validation()`)
- ✅ Debug dump (`debug_dump()`) - structured stall diagnosis with conclusion, frontiers, heads, statistics, collation, validation inventory, per-peer activity, health findings, and `finalized_pending_body` tracking
- ✅ Empty block generation - `should_generate_empty_block()`, `CollationResult` enum, `GeneratedBlockDesc`
- ✅ MC finalization callback - `SimplexSession::notify_mc_finalized()` posts to `set_mc_finalized_seqno()`
- ✅ Missing block requests - `schedule_request_candidate()` → delayed action → `receiver.request_candidate()`
- ✅ Parent metadata resolution for empty/recovery helpers - `PendingParentResolution`, `update_resolution_cache_chain()`, `find_first_missing_parent()`
- ✅ Finalized-driven delivery - `handle_block_finalized()`, `maybe_apply_finalized_state()`, `finalized_pending_body` for deferred body materialization
- ✅ Roundless listener model - round is not used for Simplex sequencing logic
- ✅ Separate session creation/start - `create_session()` + `start(initial_block_seqno)`
- ✅ Candidate chaining within leader windows (C++ parity)
- ✅ Leader window desync margin (`max_leader_window_desync`) for ingress filtering
- ✅ Block-rate cap timing parity (`min_block_interval_ms`) for validation pacing
- ✅ Standstill coordination - calls `receiver.reschedule_standstill()` on finalization, `set_standstill_slots()` on finalization/skip
- ✅ DB persistence - finalized blocks, candidate infos, notar certs, votes, pool state persisted to RocksDB
- ✅ Startup recovery - bootstrap load, vote replay, receiver cache restore, finalized-boundary restoration
- ✅ Late-join handling - finalized blocks can be known before body arrival via `finalized_pending_body`
- ⚠️ Precollation parent tracking - needs fix for cross-window scenarios

**Key methods:**
- `check_all()` - Main loop entry point, calls FSM and processes events
- `check_collation()` - Check if we should generate a block and invoke collation
- `invoke_collation(slot)` - Request block generation (or generate empty block if lag detected)
- `generated_block(CollationResult)` - Process collated/empty block, sign, broadcast, submit to FSM
- `should_generate_empty_block(seqno)` - Check if empty block needed (finalization lag)
- `create_normal_block_desc()` / `create_empty_block_desc()` - Prepare block for broadcast
- `on_vote()` - Handle incoming vote from network
- `broadcast_vote()` - Sign and send vote via receiver
- `process_delayed_actions()` - Execute scheduled closures
- `process_simplex_events()` - Dispatch FSM events to handlers
- `init_metrics()` - Initialize all metrics for performance tracking

**Metrics tracked:**
- `simplex_check_all_calls` - Counter for main loop iterations
- `simplex_process_events_calls` - Counter for FSM event processing
- `simplex_errors` - Counter for session errors (passed via `SessionStats`)
- `simplex_time:slot_duration` - Histogram for slot duration
- `simplex_time:validation_latency` - Histogram for validation time
- `simplex_time:collation_latency` - Histogram for collation time
- `simplex_active_weight` - Gauge for current network active weight
- `simplex_validates.*` - ResultStatusCounter for validation requests
- `simplex_collates.*` - ResultStatusCounter for collation completion results
- `simplex_collation_starts` - Counter for all collation entry attempts
- `simplex_commits.*` - legacy-named ResultStatusCounter for finalized-delivery/apply outcomes
- `simplex_precollation_requests` - Counter for precollation requests
- `simplex_precollation_results` - Counter for precollation completions
- `simplex_collates_precollated.*` - ResultStatusCounter for precollated block hits
- `simplex_candidate_received_broadcast` - Counter for peer-delivered broadcast candidate bodies
- `simplex_candidate_received_query` - Counter for peer-delivered query-response candidate bodies
- `simplex_skipped_slots` - Counter for skipped slots
- `simplex_batch_commits` - legacy-named batch finalized-apply metric
- `simplex_batch_commit_size` - legacy-named histogram for finalized batch size
- `simplex_finalized_pending_body_count` - Gauge for finalized blocks waiting for body arrival

### SimplexState (`simplex_state.rs`)

Core consensus state machine (crate-private):
- Implements the three-vote Simplex protocol used by C++
- Event-based output via `SimplexEvent` enum
- Vote accounting with threshold detection
- Leader window and slot management
- No external dependencies (TL, network abstracted)

**Event model**: Instead of callbacks, SimplexState produces events:
- `BroadcastVote(vote)` - Vote to broadcast to all validators
- `BlockFinalized(slot, block)` - Block finalized (triggers `on_block_finalized`)
- `SlotSkipped(slot)` - Slot skipped (handled internally, no callback)

**API**:
- `SimplexState::new(&SessionDescription)` - Create FSM
- `on_candidate(&desc, candidate)` - Process incoming block
- `on_vote(&desc, validator_idx, vote, signature, raw_vote)` - Process incoming vote
- `check_all(&desc)` - Process timeouts and pending actions
- `pull_event()` - Get next output event
- `has_pending_events()` - Query event queue (tests)
- `get_available_parent(slot)` - Get parent block for collation
- `has_available_parent(slot)` - Check if parent is available for collation
- `get_tracked_slots_interval()` - Returns `(first_non_finalized_slot, current_window_end)` for standstill
- `set_notarize_certificate(&desc, slot, block_hash, cert)` - Import external notarization certificate
- `cleanup_slots(up_to_slot)` - Clean up old slots (called externally by SessionProcessor, respects first_non_finalized_slot)
- `debug_dump(&desc, full_dump)` - Dump FSM state (compact or full)

### SessionDescription (`session_description.rs`)

Session-level constants (crate-private):
- Validator set (public keys, weights, ADNL IDs)
- Threshold calculations (1/3, 2/3)
- Leader window helpers
- Time control for log replay

### Block Types (`block.rs`)

Block candidate data structures (public module):

#### Index Newtypes (Type Safety)

```rust
SlotIndex(u32)       // Consensus slot number, Display: "s0", "s42"
WindowIndex(u32)     // Leader window index, Display: "w0", "w3"
ValidatorIndex(u32)  // Validator position, Display: "v000", "v042"
```

These prevent parameter mixing bugs and provide consistent logging output.

#### Type Hierarchy

```
RawCandidateId (hash-based, before parent resolution)
├── slot: u32           - Slot number
└── hash: UInt256       - SHA256 of TL CandidateHashData

CandidateId (resolved with full BlockIdExt)
├── slot: u32           - Slot number
├── hash: UInt256       - Same hash as RawCandidateId
└── block: BlockIdExt   - Resolved block ID

RawCandidate (from network)
├── id: RawCandidateId           - Hash-based ID
├── parent_id: Option<...>       - Parent (None for genesis)
├── leader: u32                  - Leader validator index
├── block: Option<BlockCandidate>      - None for empty blocks
├── referenced_block: Option<BlockIdExt>  - For empty: inherited BlockIdExt
└── signature: Vec<u8>           - Ed25519 signature

Candidate (resolved, parent fully known)
├── id: CandidateId         - Resolved ID
├── parent_id: Option<...>  - Resolved parent
├── block: Option<...>      - Block data (None for empty)
└── signature: Vec<u8>      - Ed25519 signature
```

#### Key Concepts

- **Empty blocks**: Have `block = None`, used for finalization recovery when chain is behind
- **Invariant 1**: Either `block.is_some()` OR `parent_id.is_some()` must be true
- **Invariant 2**: If `block.is_none()`, then `referenced_block.is_some()` (set via `new_empty()`)
- **Resolution**: `RawCandidate::resolve()` creates `Candidate`, returns `Result` for empty blocks
- **Constructors**: `Candidate::new()` validates invariants with `debug_assert!`

#### Hash Computation

Different TL types for non-empty and empty blocks (matches C++):
- **Non-empty**: `candidateHashDataOrdinary(block, collated_file_hash, parent:CandidateParent)`
- **Empty**: `candidateHashDataEmpty(block:BlockIdExt, parent:CandidateId)`

#### Serialization

- **Non-empty blocks**: `consensus.block` TL variant
- **Empty blocks**: `consensus.empty` TL variant
- **Compression**: `RawCandidate::serialize(compress: bool)` - LZ4 compression when `true`

### Receiver (`receiver.rs`)

Network overlay management (crate-private):
- Processing thread (`SXRCV:*`)
- Message deserialization and signature verification
- Vote deduplication (per-slot HashMap)
- Randomized send order (shuffled every 10s)
- Per-node statistics and metrics
- Delayed actions infrastructure (`post_delayed_action()`, `process_delayed_actions()`)
- Candidate resolver cache for query responses (`CandidateResolverCache`)
- Outbound candidate requests with retry (`request_candidate_impl()`, `handle_candidate_request_timeout()`)
- Query handler for `requestCandidate` (`handle_query()`)
- Standstill resolution aligned with C++:
  - `reschedule_standstill()` - called only on finalization (not skip)
  - `set_standstill_slots(begin, end)` - filters votes to `[first_non_finalized_slot, current_window_end)`
  - `check_standstill()` - re-broadcasts votes in tracked range
- Network metrics: `in_messages_count`, `out_messages_count`, `in_broadcasts_count`, `out_broadcasts_count`, `in_queries_count`

### Utils (`utils.rs`)

Cryptographic and utility functions:

| Function | Purpose |
|----------|---------|
| `threshold_66()` | Calculate 2/3 threshold (ceiling division) |
| `threshold_33()` | Calculate 1/3 threshold (ceiling division) |
| `create_data_to_sign()` | Create session-scoped data wrapper for signing |
| `check_session_signature()` | Verify session-scoped signature |
| `sign_with_session()` | Create session-scoped signature |
| `check_candidate_signature()` | Verify block candidate signature |
| `sign_candidate()` | Sign block candidate |
| `compute_candidate_id_hash()` | Compute candidate ID hash for non-empty blocks |
| `compute_candidate_id_hash_empty()` | Compute candidate ID hash for empty blocks |
| `extract_block_info_from_candidate()` | Extract BlockIdExt from candidate bytes |
| `bytes_to_hex()` | Format bytes as hex for trace logging |
| `sign_vote()` | Sign a vote with session-scoped signature |
| `verify_vote_signature()` | Verify vote signature |
| `extract_vote()` | Extract FSM vote from TL signed vote |

**Session-scoped signatures**: All signatures are wrapped with the session ID using `consensus.dataToSign` TL type to prevent cross-session replay attacks.

## Configuration

### SessionOptions

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `proto_version` | `u32` | 0 | Protocol version |
| `slots_per_leader_window` | `u32` | 1 | Slots per leader window |
| `target_rate` | `Duration` | 1s | Target block time |
| `first_block_timeout` | `Duration` | 3s | First block timeout |
| `timeout_increase_factor` | `f64` | 1.05 | Adaptive backoff factor |
| `max_backoff_delay` | `Duration` | 100s | Max backoff delay |
| `max_block_size` | `usize` | 4 MB | Max block size |
| `max_collated_data_size` | `usize` | 4 MB | Max collated data |
| `collation_retry_timeout` | `Duration` | 1s | Collation retry timeout |
| `collation_retry_max_attempts` | `u32` | 3 | Max collation retries |
| `use_callback_thread` | `bool` | true | Use separate callback thread |

## Integration

### Creating a Session

```rust
use simplex::{SessionFactory, SessionOptions, SessionListenerPtr, SessionNode};
use std::sync::{Arc, Weak};

// 1. Create overlay manager
let overlay = SessionFactory::create_in_process_overlay_manager(4);

// 2. Prepare validator nodes
let nodes: Vec<SessionNode> = validators.iter().map(|v| SessionNode {
    public_key: v.public_key.clone(),
    adnl_id: v.adnl_id.clone(),
    weight: v.weight,
}).collect();

// 3. Create session
let shard = ton_block::ShardIdent::masterchain();  // Or workchain shard
let session = SessionFactory::create_session(
    &SessionOptions::default(),
    &session_id,
    &shard,
    nodes,
    &local_private_key,
    "/path/to/db".into(),
    overlay,
    Arc::downgrade(&listener) as SessionListenerPtr,
)?;

// 4. Start consensus processing with expected first block seqno
let initial_block_seqno = 1;
session.start(initial_block_seqno);

// 5. Session runs in background, callbacks via SessionListener
// 6. Stop when done
session.stop();
```

### Implementing SessionListener

```rust
impl SessionListener for MyListener {
    fn on_candidate(&self, source_info, root_hash, data, collated_data, callback) {
        // Validate block candidate
        // Call callback with decision
    }

    fn on_generate_slot(&self, source_info, request, callback) {
        // Generate new block when we're leader
        // Call callback with block candidate
    }

    fn on_block_finalized(&self, block_id, round, source, root_hash, file_hash, data, signatures, approve_signatures) {
        // Finalized block delivered by Simplex (may be out of order)
    }

    fn on_block_committed(&self, source_info, root_hash, file_hash, data, signatures, approve_signatures, stats) {
        unreachable!("Simplex does not use on_block_committed(); finalized blocks arrive via on_block_finalized()");
    }

    fn on_block_skipped(&self, round: u32) {
        unreachable!("Skip events are handled internally by Simplex");
    }
}
```

## Tests

**Total: 427 tests** (406 lib + 15 integration + 6 doc-tests)

**Integration tests**: 12 consensus + 1 collation + 1 validation + 1 restart

**Crypto tests include**: Threshold calculations, session signatures, candidate signatures, vote TL serialization, vote signing with session wrapper, and signature format tests (C++ TL library compatibility).

### test_consensus.rs

Multi-instance consensus tests with in-process overlay.

**Test Serialization**: Uses `SIMPLEX_TEST_MUTEX` to prevent parallel execution of consensus tests (avoids resource conflicts).

| Test | Description | Status |
|------|-------------|--------|
| `test_simplex_consensus_basic` | Basic consensus with 7 nodes, 100 rounds | ✅ |
| `test_simplex_consensus_with_failures` | Consensus with simulated failures | ✅ |
| `test_simplex_consensus_finalcert_recovery` | FinalCert recovery and finalized delivery | ✅ |
| `test_simplex_consensus_shard_with_mc_notifications` | MC finalization forwarding to shards | ✅ |
| `test_simplex_consensus_adnl_overlay` | ADNL overlay-based consensus | ✅ |
| `test_simplex_consensus_adnl_net_gremlin` | ADNL net gremlin (packet loss/delay simulation) | ✅ |
| `test_simplex_consensus_restart_gremlin` | Restart gremlin (stop/restart with DB persistence) | ⚠️ (temp ignore) |
| `test_simplex_consensus_candidate_chaining` | Candidate chaining within leader windows | ✅ |
| `test_simplex_consensus_candidate_chaining_with_lossy_overlay` | Candidate chaining with packet loss | ✅ |
| `test_simplex_start_gate` | Session start gate (create/start separation) | ✅ |
| `test_collated_file_hash_consistency` | Collated file hash consistency checks | ✅ |
| `test_empty_collated_data_hash` | Empty collated data hash computation | ✅ |

**Test Configuration:**
- `total_slots: u32` - Number of slots to complete (default: 100)
- `min_finalized_percent: f64` - Minimum required finalized-delivery rate (default varies by test)
- `test_timeout: Duration` - Maximum time to wait
- `expect_timeout: bool` - If true, test passes on timeout

**Running:**
```bash
# Run all simplex tests
cargo test -p simplex

# Run with logging
TEST_LOGS=1 cargo test -p simplex test_simplex_consensus_basic -- --nocapture
```

### test_validation.rs

Two-node validation test validating the candidate flow.

| Test | Description |
|------|-------------|
| `test_two_node_validation` | Two nodes, validates candidate broadcast and reception |

### test_restart.rs

Restart integration tests (public API only) validating DB-backed stop/restart recovery.

| Test | Description |
|------|-------------|
| `test_single_session_restart_round_monotonicity_first_commit_after_finalized` | Restart after finalized boundary; resumed session keeps finalized state consistent via state restoration |

**Running:**
```bash
TEST_LOGS=1 cargo test -p simplex --test test_restart -- --nocapture
```

### Unit Tests (`src/tests/`)

Crate-private unit tests with access to internal symbols.

| Module | Description |
|--------|-------------|
| `test_crypto.rs` | Thresholds, session signatures, vote TL roundtrips |
| `test_block.rs` | Candidate types, newtypes (SlotIndex/ValidatorIndex/WindowIndex), empty blocks |
| `test_certificate.rs` | `voteSignatureSet` parsing, `Certificate<T>` verification, threshold checks |
| `test_database.rs` | Simplex DB records + bootstrap roundtrips |
| `test_receiver.rs` | Receiver behavior, standstill cache, certificate send/receive, candidate resolver flow |
| `test_candidate_resolver.rs` | CandidateResolverCache unit tests (late-joiner repair) |
| `test_session_processor.rs` | SessionProcessor unit tests (manual clock, delayed actions, scheduling, finalized delivery) |
| `test_restart.rs` | Restart byte-level tests (crate-private) |
| `test_simplex_state.rs` | FSM logic + invariants (included via `#[path]`) |
| `test_slot_bounds.rs` | Slot bounds validation |
| `test_misbehavior.rs` | Misbehavior proofs and invariant checks |
| `test_session_description.rs` | Validator indexing, thresholds, time control |

**Running:**
```bash
cargo test -p simplex tests::test_crypto::
cargo test -p simplex tests::test_block::
```

### SimplexState FSM Tests (`src/tests/test_simplex_state.rs`)

Core tests for the consensus state machine. Located in a separate file but included
via `#[path]` attribute in `simplex_state.rs` to access private struct fields. Tests cover:

- **Basic FSM**: State creation, initialization, validation
- **Candidate handling**: First slot with genesis, pending blocks, parent readiness / empty-tip lookup
- **Vote accounting**: Notarize/skip/finalize weights, conflict detection
- **Threshold triggers**: BlockNotarized (2/3), BlockFinalized (2/3), SlotSkipped (2/3)
- **Certificate Creation**: Notarization/finalization/skip certificates at threshold, caching, events
- **External Certificate Import**: `set_notarize_certificate()` updates vote accounting and flags
- **Parent validation**: notarized/finalized parent readiness for collation
- **Misbehavior detection**: conflicting votes and invalid ranges
- **Corner cases**: Finalized slot handling, window cleanup, duplicate votes, multiple blocks per slot

**Running:**
```bash
cargo test -p simplex simplex_state::tests::
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| `consensus-common` | Shared types (Session, SessionListener, SessionNode), overlay interfaces, compression utilities |
| `ton_api` | TL serialization for protocol messages |
| `crossbeam` | Task queue channels |

## Protocol Messages

TL schema messages from `tl/ton_api/tl/ton_api.tl`:

| Message | Purpose |
|---------|---------|
| `consensus.overlayId` | Overlay identification (session_id + nodes) |
| `consensus.dataToSign` | Session-scoped signature wrapper |
| `consensus.candidateId` | Candidate identification (slot + hash) |
| `consensus.candidateParent` | Parent reference (wraps CandidateId) |
| `consensus.candidateWithoutParents` | Marker for genesis/first block |
| `consensus.candidateHashDataOrdinary` | Hash data for non-empty blocks |
| `consensus.candidateHashDataEmpty` | Hash data for empty blocks |
| `consensus.block` | Non-empty block candidate data |
| `consensus.empty` | Empty block candidate data |
| `consensus.simplex.vote` | Vote wrapper with signature |
| `consensus.simplex.notarizeVote` | Notarization vote (on wire) |
| `consensus.simplex.finalizeVote` | Finalization vote (on wire) |
| `consensus.simplex.skipVote` | Skip vote (on wire) |
| `consensus.simplex.voteSignature` | Validator signature in certificate |
| `consensus.simplex.voteSignatureSet` | Aggregated signatures |
| `consensus.simplex.certificate` | Vote + signatures (for queries) |
| `consensus.simplex.candidateAndCert` | Candidate + notarization cert (query response) |
| `consensus.simplex.requestCandidate` | Query for missing candidate (RPC) |

### Signature Scheme

All signatures are **session-scoped** to prevent cross-session replay:

```
signature = Ed25519.sign(private_key, serialize(consensus.dataToSign(session_id, data)))
```

For candidates, the signed data depends on block type:
- **Non-empty blocks**: `consensus.candidateHashDataOrdinary(block, collated_file_hash, parent)`
- **Empty blocks**: `consensus.candidateHashDataEmpty(block, parent_id)`

## Telemetry and Health Checks

### Metrics Catalog

All metrics use the `simplex_` prefix. Latency histograms use `time:` prefix (values in milliseconds).

#### Counters

| Metric | Description | Update Point |
|--------|-------------|--------------|
| `simplex_check_all_calls` | Main loop iterations | `check_all()` |
| `simplex_process_events_calls` | FSM event processing calls | `process_simplex_events()` |
| `simplex_errors` | Protocol-breaking errors | `increment_error()` |
| `simplex_misbehavior` | Detected misbehavior events | `on_vote()` conflict detection |
| `simplex_batch_commits` | Legacy batch finalized-apply metric | historical naming; sequential commit scheduler removed |
| `simplex_skip_total` | Total slot skip events | `handle_slot_skipped()` |
| `simplex_votes_in_notarize` | Inbound notarize votes | `on_vote()` |
| `simplex_votes_in_finalize` | Inbound finalize votes | `on_vote()` |
| `simplex_votes_in_skip` | Inbound skip votes | `on_vote()` |
| `simplex_votes_out_notarize` | Outbound notarize votes | `broadcast_vote()` |
| `simplex_votes_out_finalize` | Outbound finalize votes | `broadcast_vote()` |
| `simplex_votes_out_skip` | Outbound skip votes | `broadcast_vote()` |
| `simplex_certs_in` | Verified inbound certificates | `on_certificate()` |
| `simplex_certs_relayed` | Certificates relayed to peers | `handle_*_reached()` |
| `simplex_cert_conflict` | Certificate storage conflicts | `on_certificate()` |
| `simplex_cert_verify_fail` | Certificate verification failures | `on_certificate()` |
| `simplex_validation_reject` | Validation rejections | `candidate_decision_fail()` |
| `simplex_validation_late_callback` | Late validation callbacks | `candidate_decision_ok/fail()` |
| `simplex_health_warnings` | Health anomaly warnings (not errors) | `run_health_checks()` |
| `simplex_candidate_received_broadcast` | Peer-delivered broadcast candidate bodies (excludes local self-loop) | `on_candidate_received()` |
| `simplex_candidate_received_query` | Peer-delivered requestCandidate/query-response candidate bodies (excludes local self-loop) | `on_candidate_received()` |
| `simplex_collation_starts` | Unified collation entry attempts across async, retry, precollated, and empty-block paths | `check_collation()`, `invoke_collation()`, `invoke_collation_retry()` |
| `simplex_precollation_requests` | Precollation requests sent | `invoke_precollation()` |
| `simplex_precollation_results` | Precollation results received | `precollation_result()` |

#### ResultStatusCounters (auto-generate `.total`/`.success`/`.failure`)

| Metric | Description |
|--------|-------------|
| `simplex_validates` | Block validation results |
| `simplex_collates` | Block collation completion results (`.total` only covers async listener requests) |
| `simplex_commits` | Finalized-delivery/apply results (legacy metric family name) |
| `simplex_collates_precollated` | Precollated block hits |
| `simplex_collates_expire` | Expired collation time slots |

#### Gauges

| Metric | Description | Update Point |
|--------|-------------|--------------|
| `simplex_active_weight` | Active validator weight | `check_all()` |
| `simplex_total_weight` | Total validator weight | `init_metrics()` |
| `simplex_threshold_66` | 2/3 weight threshold | `init_metrics()` |
| `simplex_last_finalized_slot` | Last finalized slot index | `maybe_apply_finalized_state()` |
| `simplex_finalized_pending_body_count` | Finalized blocks waiting for body arrival | `handle_block_finalized()`, cleanup, materialization |
| `simplex_first_non_finalized_slot` | First non-finalized slot (FSM) | `check_all()` |
| `simplex_first_non_progressed_slot` | First non-progressed slot (FSM) | `check_all()` |

#### Histograms

| Metric | Unit | Description |
|--------|------|-------------|
| `time:slot_duration` | ms | Time from slot start to finalization |
| `time:validation_latency` | ms | Block validation callback latency |
| `time:collation_latency` | ms | Block generation latency |
| `time:broadcast_validation_latency` | ms | Network receive to validation complete |
| `time:slot_stage1_received_latency` | ms | Slot start to first candidate received |
| `time:slot_stage2_notarized_latency` | ms | Slot start to first notarize vote |
| `time:slot_stage3_finalized_latency` | ms | Slot start to first finalize vote |
| `simplex_batch_commit_size` | count | Blocks applied per finalized batch (legacy metric name) |

#### Receiver Counters

| Metric | Description |
|--------|-------------|
| `simplex_receiver_in_messages_bytes` | Inbound message bytes |
| `simplex_receiver_out_messages_bytes` | Outbound message bytes |
| `simplex_receiver_in_broadcasts_bytes` | Inbound broadcast bytes |
| `simplex_receiver_out_broadcasts_bytes` | Outbound broadcast bytes |
| `simplex_receiver_in_bytes` | Total inbound bytes |
| `simplex_receiver_out_bytes` | Total outbound bytes |
| `simplex_receiver_in_messages_count` | Inbound message count |
| `simplex_receiver_out_messages_count` | Outbound message count |
| `simplex_receiver_in_broadcasts_count` | Inbound broadcast count |
| `simplex_receiver_out_broadcasts_count` | Outbound broadcast count |
| `simplex_receiver_in_queries_count` | Inbound query count |
| `simplex_candidate_requests` | Candidate requests initiated |
| `simplex_candidate_request_retries` | Candidate request retries |
| `simplex_candidate_request_timeouts` | Candidate request timeouts |
| `simplex_candidate_request_giveups` | Candidate request give-ups |
| `simplex_standstill_triggers` | Standstill detection triggers |
| `simplex_standstill_votes_rebroadcast` | Votes rebroadcast on standstill |
| `simplex_standstill_certs_rebroadcast` | Certs rebroadcast on standstill |

### Derivative Metrics

All counters and progress gauges are registered as derivative metrics via `MetricsDumper`. The dumper computes `/s` rate between periodic dumps (session: 15s, receiver: 30s).

**Key progression indicators** (non-zero speed = healthy, zero = stalled):

- `simplex_last_finalized_slot` -- finalized slots per second
- `simplex_first_non_finalized_slot` -- FSM advancement rate
- `simplex_commits.total` -- finalized-delivery throughput (legacy metric name)
- `simplex_validates.total` -- validation throughput
- `simplex_collation_starts` -- collation entry attempts per second
- `simplex_candidate_received_broadcast` + `simplex_candidate_received_query` -- peer-delivered candidate-body ingress rate (sum them for total ingress)

### Health Checks

Health checks run every 20 seconds. Anomaly alerts use the `SIMPLEX_HEALTH` log prefix for easy grep/monitoring integration. Health warnings increment `simplex_health_warnings` but **not** `simplex_errors` (preserving test semantics where `total_errors == 0` is asserted).

| Anomaly | Severity | Condition | Suggested Action |
|---------|----------|-----------|------------------|
| `progress_gap` | WARN/ERROR | `first_non_progressed - first_non_finalized > window_size` | Check network connectivity |
| `zero_finalization_speed` | WARN (>15s) / ERROR (>60s) | No new finalized slots | Check validator activity, standstill |
| `low_activity` | WARN (<66%) / ERROR (<33%) | Active weight below threshold | Check peer connectivity |

**Log format** (single-line, grep-friendly):

```
SIMPLEX_HEALTH anomaly=<type> session=<8-char-hex> <key>=<value> ...
```

### Session Debug Dump

Every 15–20 seconds the session produces a structured dump. Under normal operation the dump goes to DEBUG level (`dump [OK]`). When no finalizations occur for `ROUND_DEBUG_PERIOD` (15s), the dump fires at ERROR level (`dump [STALLED]`) with a stall conclusion.

**Health status line** (INFO, always emitted):
```
Session 882cc37b health [OK]: shard=-1:8000000000000000 slot_nf=s57 slot_np=s57 finalized_head_seqno=43
```

**Stalled dump structure** (ERROR level):
```
Session <full_session_id> dump [STALLED]:
  conclusion:
    - <HealthFindingKind>: <summary>
  shard=<shard_id>
  header:
    validators=N local=vNNN session_time=Xs slot_duration=Xs
    total_weight=W th66=T th33=T active_weight=W (XX.X%)
  frontiers:
    first_non_finalized=sN (unchanged Xs)
    first_non_progressed=sN (unchanged Xs)
    last_finalization: seqno=N slot=sN, Xs ago
    last_notarization: seqno=? slot=sN, Xs ago
    last_final_cert: seqno=? slot=sN, Xs ago
    last_notar_cert: seqno=? slot=sN, Xs ago
  heads:
    finalized_head_seqno=N
    finalized_head=slot sN id=((shard, seqno, rh ..., fh ...))
    last_mc_applied=((shard, seqno, rh ..., fh ...))
  statistics:
    candidates: received=N validated=N (%) notarized=N (%) finalized=N (%) other=N (%)
    traffic: msgs_in=N msgs_out=N bcasts_in=N bcasts_out=N
    votes_in: notar=N final=N skip=N
    duplicates: votes=N broadcasts=N request_candidates_sent=N request_candidates_recv=N
  collation:
    window wN slots=[sN..sN] leader=vN pubkey_b64=... adnl_b64=...
      sN phase=<SlotWaitPhase> reason=... notar=N% final=N% skip=N% flags=[...] certs=[...]
  validation:
    received (N%): ...
    validated (N%): ...
    notarized (N%): ...
    finalized (N%): ...     (last 10s only)
    other: omitted=N total_received=N
  peers:
    vN adnl_b64=... pubkey_b64=... weight=N (N%) last_activity=Xs ago ...
  health_findings:
    - [Warn|Error] <kind>: <summary>
  standstill_diagnostic: ...
```

**`SlotWaitPhase` values** identify what the system is waiting for in each non-finalized slot:
`WaitingForCandidate`, `WaitingForParentBase`, `WaitingForNotarization`, `NotarizedWaitingForFinalization`, `TimeoutSkipped`, `Skipped`, `Finalized`.

### Metrics Dump Format

Periodic dumps output all registered metrics with current values, derivative speeds, and computed percentages. Example:

```
simplex_last_finalized_slot       42     0.28/s
simplex_commits.total             42     0.28/s
simplex_votes_in_notarize        126     0.84/s
```

## References

- [TON C++ Implementation](https://github.com/ton-blockchain/ton) (`testnet/validator/consensus/simplex`)

## License

Copyright (C) 2025-2026 RSquad Blockchain Lab. All Rights Reserved.
