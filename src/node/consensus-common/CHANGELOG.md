# Changelog

All notable changes to the Consensus Common library will be documented in this file.

---

## [0.2.0] - 2026-01-18

### Added

#### Async Key-Value Storage (`lib.rs`, `async_key_value_storage.rs`)

RocksDB-based async storage for consensus persistence, inspired by C++ `td/db/KeyValueAsync.h`.

- **Types (in `lib.rs`)**
  - `AsyncKeyValueStorage` - Trait for async key-value operations
  - `AsyncKeyValueStoragePtr` - `Arc<dyn AsyncKeyValueStorage>`
  - `AsyncKeyValueStorageOptions` - Configuration (use_callback_thread)
  - `StorageKey`, `StorageValue` - Key/value types (`Vec<u8>`)
  - `StorageAsyncResult<T>` - Async result trait with `is_ready()`, `try_get()`, `wait()`
  - `StorageWriteCallback`, `StorageGetCallback`, `StoragePrefixScanCallback` - Callbacks
  - `contains()` - Inline implementation via `get()`, returns `StorageAsyncResultPtr<bool>`

- **Implementation (in `async_key_value_storage.rs`, crate-private)**
  - `RocksDbAsyncKeyValueStorage` - RocksDB-based implementation
  - Background DB processing thread (`kv-db:{id}`)
  - Optional callback thread (`kv-callback:{id}`)
  - Prefix scanning for TL-style keys
  - Periodic metrics dump (30s)
  - Mark-for-destroy lifecycle management
  - Trace logging with target `consensus_storage`
  - Periodic stop/drop logging for debugging
  - DRY: `sync()` uses `StorageAsyncResultImpl` for both DB and callback queue drain
  - No Mutex for thread handles (only accessed in Drop with `&mut self`)

- **Factory**
  - `ConsensusCommonFactory::create_async_key_value_storage(path, id, opts)` - Create storage

- **Tests**
  - 22 unit tests in `src/tests/test_async_key_value_storage.rs`

### Threading Model

```
Caller Thread ──post_task()──▶ DB Processing Thread ──callback──▶ Callback Thread
                                      │                                │
                                      ▼                                ▼
                                 RocksDB                    User callbacks
```

- **Caller thread**: Posts operations via `crossbeam`, receives `StorageAsyncResult`
- **DB Processing thread**: Opens RocksDB, processes operations, posts callbacks
- **Callback thread** (optional): Executes user callbacks without blocking DB

### Reference

Based on C++ `tddb/td/db/KeyValueAsync.h` from ton-node-cpp-simplex.

---

## [0.1.0] - 2026-01-07

### Added

#### Core Types (`lib.rs`)
- **Type Aliases**
  - `PublicKey`, `PublicKeyHash`, `PrivateKey` - Cryptographic key types
  - `SessionId`, `BlockHash`, `BlockSignature` - Block identification types
  - `RawBuffer`, `ValidatorWeight`, `ValidatorBlockId` - Utility types
  - `Result<T>` - Standard result type alias

- **Structures**
  - `ConsensusNode` - Validator node description (ADNL ID, public key)
  - `SessionStats` - Session statistics for committed blocks
  - `LogReplayOptions` - Log replay configuration

- **Traits**
  - `BlockPayload` - Interface for block payload data
  - `ConsensusOverlay` - Outgoing overlay interface (messages, queries, broadcasts)
  - `ConsensusOverlayListener` - Inbound overlay interface
  - `ConsensusOverlayManager` - Overlay factory interface
  - `ConsensusOverlayLogReplayListener` - Time control for log replay
  - `ActivityNode` - Liveness tracking interface
  - `ConsensusReplayListener` - Replay event callbacks
  - `LogPlayer` - Log replay interface

- **Factory**
  - `ConsensusCommonFactory` with methods:
    - `create_block_payload()` / `create_empty_block_payload()`
    - `create_activity_node()`
    - `create_dummy_overlay_manager()`
    - `create_in_process_overlay_manager()`
    - `create_adnl_overlay_manager()`
    - `create_log_player()` / `create_log_players()`

#### Internal Modules (crate-private)
- **activity_node.rs**
  - `ActivityNodeManager` - Activity tracking implementation
  - Timestamp-based liveness detection

- **block_payload.rs**
  - `BlockPayloadImpl` - BlockPayload trait implementation
  - Creation time tracking

- **adnl_overlay.rs**
  - `AdnlOverlayManager` - ADNL-based overlay for production
  - Network stack integration
  - Broadcast with configurable hops

- **in_process_overlay.rs**
  - `OverlayManagerImpl` - In-process overlay for integration tests
  - Multi-threaded message routing
  - No network I/O

- **dummy_catchain_overlay.rs**
  - `DummyConsensusOverlayManager` - Dummy overlay for unit tests
  - No-op implementations

- **log_player.rs**
  - `LogPlayerImpl` - Log replay implementation
  - Session enumeration from log files
  - Time-controlled replay

#### Public Modules
- **compression.rs**
  - `compress_candidate_data()` - LZ4 compress block + collated data into single BOC
  - `decompress_candidate_data()` - Decompress back to block and collated data
  - Used by validator-session and simplex for block candidate serialization

- **profiling.rs**
  - `check_execution_time!` macro for timing code blocks
  - `instrument` macro for function instrumentation
  - Metrics integration

- **utils.rs**
  - `serialize_tl_bare_object!` macro
  - `serialize_tl_boxed_object!` macro
  - Re-exports from `ton_api`

#### Test Utilities (feature: test-utils)
- **node_test_network.rs**
  - `NodeTestNetwork` - Test network configuration
  - `Node` - Test node wrapper
  - Network setup utilities

#### Feature Flags
- `telemetry` (default) - Enable telemetry collection
- `export_key` - Enable key export functionality
- `test-utils` - Enable test utilities module

### Notes

This crate was created by extracting common code from the `catchain` crate to enable
sharing between different consensus implementations (catchain-based validator-session
and simplex). The `catchain` crate now imports these types and re-exports them with
`Catchain*` prefixes for backward compatibility:

| consensus-common | catchain/validator-session |
|------------------|----------------------------|
| `ConsensusNode` | `CatchainNode` |
| `ConsensusOverlay` | `CatchainOverlay` |
| `ConsensusOverlayListener` | `CatchainOverlayListener` |
| `ConsensusOverlayManager` | `CatchainOverlayManager` |
| `ConsensusOverlayPtr` | `CatchainOverlayPtr` |
| `ActivityNode` | `ActivityNode` (no alias) |
| `BlockPayload` | `BlockPayload` (no alias) |
| `compression::compress_candidate_data` | re-exported by validator-session |
| `compression::decompress_candidate_data` | re-exported by validator-session |

---

## Version History

| Version | Date | Tag | Description |
|---------|------|-----|-------------|
| 0.2.0 | 2026-01-18 | `consensus-common-0.2.0` | Async key-value storage |
| 0.1.0 | 2026-01-07 | `consensus-common-0.1.0` | Initial extraction from catchain |

---

[Unreleased]: https://github.com/RSquad/ton-node/compare/consensus-common-0.2.0...HEAD
[0.2.0]: https://github.com/RSquad/ton-node/releases/tag/consensus-common-0.2.0
[0.1.0]: https://github.com/RSquad/ton-node/releases/tag/consensus-common-0.1.0
