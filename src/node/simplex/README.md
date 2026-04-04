# Simplex Consensus Protocol

**Version**: 0.5.0 (March 20, 2026) | [Changelog](CHANGELOG.md)

Rust implementation of the Simplex consensus protocol for TON blockchain.

> **C++ Reference**: Primary tracking is `ton-blockchain/ton/tree/simplex` (main repo).  
> Secondary: `DanShaders/ton/tree/alpenglow` (superseded).

## Overview

Simplex is a consensus protocol based on the Solana Alpenglow White Paper (May 2025 v1) with modifications for TON:

- **Conservative path only** (no fast finality/optimistic path)
- **Fault tolerance**: <1/3 Byzantine nodes (not 20% as in original Alpenglow)
- **Certificate threshold**: 2/3 stake weight
- **No erasure coding**: Simple broadcast instead of Rotor shreds

### Key Design Decisions

1. **Simplified Alpenglow**: Focus on conservative path for reliability over speed
2. **Ed25519 signatures**: Individual signatures, no BLS aggregation
3. **Actor model**: Separate threads for consensus, callbacks, and network
4. **Task queues**: Cross-thread communication via closures

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

This crate targets wire-compatibility with the upstream **C++ Simplex** implementation (`origin/testnet@e40d0e36`, Feb 28, 2026).

### Protocol parity gaps (from C++ upstream)

- C++ proactively rebroadcasts FinalCerts (`cfd8850c`) — Rust standstill replay is less aggressive. **HIGH**

### Implementation parity gaps

- Committed-parent validation gate — needs state-root caching / apply-block-to-state.
- Base selection should use "max available base" like C++ `SlotState::add_available_base` (audit needed).
- C++ has `ImprovedStructureLZ4WithState` (BOC compression algo 2) — Rust only supports algos 0 and 1.
- C++ has `StoreCellHint` for DB commit optimization during MerkleUpdate apply — Rust lacks equivalent.
- C++ overlay manager can buffer messages for unknown overlays (disabled by default) — Rust lacks equivalent.

### Resolved (for reference)

- Candidate signature now signs bare `consensus.candidateId` directly, matching C++ testnet. Regression test: `test_candidate_id_to_sign_is_bare_candidate_id`.
- MC stale-head rejection implemented in `validator_group.rs` (`should_reject_stale_mc_candidate`), matching C++ `block-validator.cpp` commit `9aac62b8`.
- Adaptive first-block timeout backoff after skip implemented in `simplex_state.rs` (`apply_adaptive_timeout_backoff`), matching C++ `consensus.cpp`.
- Twostep FEC broadcast implemented in `consensus-common/adnl_overlay.rs` (`BroadcastTwostepSimple`), with C++-compatible signing.
- QUIC transport supported via `SessionOptions::use_quic` and `OverlayTransportType::SimplexQuic`. Tested in `test_adnl_overlay_quic_delivery`.
- Overlay ID computation (node ordering, short ID)
- `candidateAndCert.notar` encoding (voteSignatureSet)
- Handle incoming `consensus.simplex.certificate` on vote channel
- `requestCandidate2` removed — replaced by `get_committed_candidate`
- Shard `before_split` empty block rule
- Restart support (DB persistence + startup recovery)
- Certificate rebroadcast on restart


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
│  │  │  - vote tracking             │  │   │    - on_block_committed          │ │
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
│  3. Emit events (BlockNotarized, SafeToNotar, etc.)                         │
│  4. May broadcast new vote via Receiver                                     │
└─────────────────────────────────────────────────────────────────────────────┘
                        │
                        │ callback (if use_callback_thread)
                        ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│ SessionListener (implemented by caller)                                     │
│  - on_candidate: validate block                                             │
│  - on_generate_slot: create new block                                       │
│  - on_block_committed: finalization notification                            │
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
| `NotarizeFallbackVote` | ❌ | Fallback notarization (internal only, see below) |
| `SkipFallbackVote` | ❌ | Fallback skip (internal only, see below) |

#### Alpenglow vs TON Implementation Vote Differences

The original Alpenglow White Paper specifies 5 vote types for consensus. However, the **TON C++ implementation uses only 3 vote types on the wire**:

| Aspect | Alpenglow (Paper) | TON Implementation |
|--------|-------------------|-------------------|
| **Wire Vote Types** | 5 (Notarize, Finalize, Skip, NotarizeFallback, SkipFallback) | 3 (Notarize, Finalize, Skip) |
| **Fallback Votes** | Required for liveness under adversarial conditions | Internal FSM state only, not transmitted |
| **SafeToNotar Trigger** | Broadcasts `NotarizeFallbackVote` | Used internally for state tracking |
| **SafeToSkip Trigger** | Broadcasts `SkipFallbackVote` | Used internally for state tracking |
| **Simultaneous Voting** | N/A | C++ allows simultaneous Skip + Notarize per validator |

**Configuration**: The `enable_fallback_protocol` option in `SimplexState` controls whether fallback votes are generated internally:
- `false` (default): C++ compatible, 3 vote types on wire
- `true`: Full Alpenglow algorithm with internal fallback tracking

### Certificates

When 2/3 stake weight is reached for a vote type, a certificate is formed:
- **NotarizationCert**: Block is notarized
- **FinalizationCert**: Block is finalized (committed)
- **SkipCert**: Slot is skipped

Certificates are implicit (derived from vote counts), not explicit on-wire objects.

### Empty Blocks (TON-Specific Extension)

Empty blocks are a **finalization recovery** mechanism not in the original Alpenglow White Paper:

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
| 1/3 (33%) | `(total + 2) / 3` | Safety conditions (SafeToNotar, SafeToSkip) |

### Consensus Loop

Each slot follows this flow:

```
Collate → Broadcast → Validate → Notarize → Vote → Collect → Finalize → Commit → next slot
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
| **Commit** | `handle_block_finalized()` | `BlockFinalized` event | `on_block_committed()` |

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
│   ├── database.rs            # DB persistence for restart/recommit (crate-private)
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
│       └── test_simplex_state.rs
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
| `SimplexSession` | Simplex-specific session operations (extends `Session`) |
| `SessionListener` | Callback trait (from consensus-common) |
| `SessionStats` | Session health metrics passed to `on_block_committed` |
| `Receiver` | Network sender interface (trait) |
| `ReceiverListener` | Network receiver callbacks (trait) |

**SimplexSession Trait** (for MC finalization notification):

```rust
pub trait SimplexSession: Session {
    /// Notify session about masterchain finalization (for shard empty block decisions)
    fn notify_mc_finalized(&self, mc_block_seqno: u32);
}
```

This separate trait allows simplex-specific operations without modifying the shared `Session` trait
from validator-session. For shard chains, the higher layer (ValidatorManager) should call
`notify_mc_finalized()` when masterchain blocks are finalized to enable empty block generation.

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
- ✅ Debug dump (`debug_dump()`) - session and FSM state dump
- ✅ Empty block generation - `should_generate_empty_block()`, `CollationResult` enum, `GeneratedBlockDesc`
- ✅ MC finalization callback - `SimplexSession::notify_mc_finalized()` posts to `set_mc_finalized_seqno()`
- ✅ Missing block requests - `schedule_request_candidate()` → delayed action → `receiver.request_candidate()`
- ✅ Recursive parent resolution - `PendingParentResolution`, `update_resolution_cache_chain()`, `find_first_missing_parent()`
- ✅ Round tracking - `current_round` field tracks sequential commit counter (independent of slots)
- ✅ Standstill coordination - calls `receiver.reschedule_standstill()` on finalization, `set_standstill_slots()` on finalization/skip
- ✅ DB persistence - finalized blocks, candidate infos, notar certs, votes, pool state persisted to RocksDB
- ✅ Startup recovery - bootstrap load, vote replay, receiver cache restore, recommit to ValidatorGroup
- ✅ Download committed block via full-node proof for MC gap recovery (replaces requestCandidate2)
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
- `simplex_collates.*` - ResultStatusCounter for collation requests
- `simplex_commits.*` - ResultStatusCounter for commit requests
- `simplex_precollation_requests` - Counter for precollation requests
- `simplex_precollation_results` - Counter for precollation completions
- `simplex_collates_precollated.*` - ResultStatusCounter for precollated block hits
- `simplex_skipped_slots` - Counter for skipped slots
- `simplex_batch_commits` - Counter for batch commit operations
- `simplex_batch_commit_size` - Histogram for batch commit sizes
- `simplex_current_round` - Gauge for current round (sequential commit counter)

### SimplexState (`simplex_state.rs`)

Core consensus state machine (crate-private):
- Implements Alpenglow White Paper Algorithm 1 and 2
- Event-based output via `SimplexEvent` enum
- Vote accounting with threshold detection
- Leader window and slot management
- No external dependencies (TL, network abstracted)

**Event model**: Instead of callbacks, SimplexState produces events:
- `BroadcastVote(vote)` - Vote to broadcast to all validators
- `BlockFinalized(slot, block)` - Block finalized (triggers `on_block_committed`)
- `SlotSkipped(slot)` - Slot skipped (handled internally, no callback)

**API**:
- `SimplexState::new(&SessionDescription)` - Create FSM
- `on_candidate(&desc, candidate)` - Process incoming block
- `on_vote(&desc, validator_idx, vote)` - Process incoming vote
- `check_all(&desc)` - Process timeouts and pending actions
- `pull_event()` - Get next output event
- `pending_event_count()` / `has_pending_events()` - Query event queue
- `get_available_parent(slot)` - Get parent block for collation
- `has_available_parent(slot)` - Check if parent is available for collation
- `get_tracked_slots_interval()` - Returns `(first_non_finalized_slot, current_window_end)` for standstill
- `set_notarize_certificate(&desc, slot, block_hash, cert)` - Import external notarization certificate
- `cleanup_slots(up_to_slot)` - Clean up old slots (called externally by SessionProcessor, respects first_non_finalized_slot)
- `debug_dump(&desc, full_dump)` - Dump FSM state (compact or full)

**Options** (`SimplexStateOptions`):
- `enable_fallback_protocol` - Enable fallback votes (default: false, C++ compatible)
- `allow_skip_after_notarize` - Allow skip after notarize (default: true)
- `require_finalized_parent` - When true, parent must be finalized; when false (default, C++ mode), notarized parent OK

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
let initial_block_seqno = 1;  // Expected seqno for first block
let session = SessionFactory::create_session(
    &SessionOptions::default(),
    &session_id,
    &shard,
    initial_block_seqno,  // First block will have this seqno
    nodes,
    &local_private_key,
    "/path/to/db".into(),
    overlay,
    Arc::downgrade(&listener) as SessionListenerPtr,
)?;

// 4. Session runs in background, callbacks via SessionListener
// 5. Stop when done
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

    fn on_block_committed(&self, source_info, root_hash, file_hash, data, signatures, approve_signatures, stats) {
        // Block was finalized in consensus
    }

    fn on_block_skipped(&self, round: u32) {
        // DEPRECATED: Not called in production - skip events are handled internally.
        // Test implementations should use unreachable!() here.
    }
}
```

### Creating a Receiver (for Testing)

For unit testing the receiver component in isolation:

```rust
use simplex::{SessionFactory, ReceiverListenerPtr};

let shard = ton_block::ShardIdent::masterchain();
let max_candidate_size = 8 << 20;  // 8 MB

let receiver = SessionFactory::create_receiver(
    session_id,
    &shard,
    max_candidate_size,
    &nodes,
    &local_private_key,
    overlay_manager,
    receiver_listener_weak,
)?;

// Send votes and broadcasts
receiver.send_vote(vote);
receiver.send_block_broadcast(candidate);

// Stop when done
receiver.stop();
```

## Tests

**Total: 300 tests** (281 lib + 13 integration + 6 doc-tests)

**Integration tests**: 9 consensus + 1 collation + 1 validation + 2 restart

**Crypto tests include**: Threshold calculations, session signatures, candidate signatures, vote TL serialization, vote signing with session wrapper, and signature format tests (C++ TL library compatibility).

### test_consensus.rs

Multi-instance consensus tests with in-process overlay.

**Test Serialization**: Uses `SIMPLEX_TEST_MUTEX` to prevent parallel execution of consensus tests (avoids resource conflicts).

| Test | Description | Status |
|------|-------------|--------|
| `test_simplex_consensus_basic` | Basic consensus with 7 nodes, 100 rounds | ✅ |
| `test_simplex_consensus_with_failures` | Consensus with simulated failures | ✅ |
| `test_simplex_consensus_finalcert_recovery` | FinalCert recovery via `get_committed_candidate` | ✅ |
| `test_simplex_consensus_shard_with_mc_notifications` | MC finalization forwarding to shards | ✅ |
| `test_simplex_consensus_adnl_overlay` | ADNL overlay-based consensus | ✅ |
| `test_simplex_consensus_adnl_net_gremlin` | ADNL net gremlin (packet loss/delay simulation) | ✅ |
| `test_simplex_consensus_restart_gremlin` | Restart gremlin (stop/restart with DB persistence) | ✅ |
| `test_collated_file_hash_consistency` | Collated file hash consistency checks | ✅ |
| `test_empty_collated_data_hash` | Empty collated data hash computation | ✅ |

**Test Configuration:**
- `total_slots: u32` - Number of slots to complete (default: 100)
- `min_commit_percent: f64` - Minimum required commit rate (default: 0.5 = 50%)
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
| `test_single_session_restart_round_monotonicity_full_replay` | Restart with full replay; round/slot stream remains monotonic |
| `test_single_session_restart_round_monotonicity_first_commit_after_finalized` | Restart after finalized boundary; first post-restart commit keeps monotonicity |

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
| `test_session_processor.rs` | SessionProcessor unit tests (manual clock, delayed actions, scheduling) |
| `test_restart.rs` | Restart byte-level tests (crate-private) |
| `test_simplex_state.rs` | FSM logic + invariants (included via `#[path]`) |
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
- **Candidate handling**: First slot with genesis, pending blocks, parent resolution
- **Vote accounting**: Notarize/skip/finalize weights, conflict detection
- **Threshold triggers**: BlockNotarized (2/3), SafeToNotar (1/3), BlockFinalized (2/3)
- **Certificate Creation**: Notarization/finalization/skip certificates at threshold, caching, events
- **External Certificate Import**: `set_notarize_certificate()` updates vote accounting and flags
- **Parent Validation Modes**: `require_finalized_parent` flag tests for notarized vs finalized parent requirements and deadlock scenarios
- **Misbehavior detection**: Conflicting votes, too many fallback votes, invalid ranges
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

**Note**: Fallback votes (`NotarizeFallbackVote`, `SkipFallbackVote`) are **internal FSM state only** and have no TL representation on the wire. See "Alpenglow vs TON Implementation Vote Differences" above.

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
| `simplex_batch_commits` | Batch commit operations | `try_commit_finalized_chains()` |
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
| `simplex_precollation_requests` | Precollation requests sent | `invoke_precollation()` |
| `simplex_precollation_results` | Precollation results received | `precollation_result()` |

#### ResultStatusCounters (auto-generate `.total`/`.success`/`.failure`)

| Metric | Description |
|--------|-------------|
| `simplex_validates` | Block validation results |
| `simplex_collates` | Block collation results |
| `simplex_commits` | Block commit results |
| `simplex_collates_precollated` | Precollated block hits |
| `simplex_collates_expire` | Expired collation time slots |

#### Gauges

| Metric | Description | Update Point |
|--------|-------------|--------------|
| `simplex_active_weight` | Active validator weight | `check_all()` |
| `simplex_total_weight` | Total validator weight | `init_metrics()` |
| `simplex_threshold_66` | 2/3 weight threshold | `init_metrics()` |
| `simplex_last_finalized_slot` | Last finalized slot index | `commit_single_block()` |
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
| `simplex_batch_commit_size` | count | Blocks committed per batch |

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
- `simplex_commits.total` -- commit throughput
- `simplex_validates.total` -- validation throughput

### Health Checks

Health checks run every 20 seconds. Anomaly alerts use the `SIMPLEX_HEALTH` log prefix for easy grep/monitoring integration. Health warnings increment `simplex_health_warnings` but **not** `simplex_errors` (preserving test semantics where `total_errors == 0` is asserted).

| Anomaly | Severity | Condition | Suggested Action |
|---------|----------|-----------|------------------|
| `progress_gap` | WARN/ERROR | `first_non_progressed - first_non_finalized > window_size` | Check network connectivity |
| `zero_finalization_speed` | WARN (>15s) / ERROR (>60s) | No new finalized slots | Check validator activity, standstill |
| `low_activity` | WARN (<66%) / ERROR (<33%) | Active weight below threshold | Check peer connectivity |
| `parent_aging` | WARN (>30s) / ERROR (>120s) | Oldest pending parent resolution age | Check candidate availability |

**Log format** (single-line, grep-friendly):

```
SIMPLEX_HEALTH anomaly=<type> session=<8-char-hex> <key>=<value> ...
```

### Metrics Dump Format

Periodic dumps output all registered metrics with current values, derivative speeds, and computed percentages. Example:

```
simplex_last_finalized_slot       42     0.28/s
simplex_commits.total             42     0.28/s
simplex_votes_in_notarize        126     0.84/s
```

## References

- [Solana Alpenglow White Paper v1, May 2025](https://drive.google.com/file/d/1y_7ddr8oNOknTQYHzXeeMD2ProQ0WjMs/view)
- [Solana Alpenglow White Paper v1.1, July 2025](https://drive.google.com/file/d/1Rlr3PdHsBmPahOInP6-Pl0bMzdayltdV/view)
  - **Note**: v1.1 is a documentation update only; core algorithms unchanged
- [TON C++ Implementation](https://github.com/ton-blockchain/ton/tree/testnet/validator/consensus)

## License

Copyright (C) 2025-2026 RSquad Blockchain Lab. All Rights Reserved.
