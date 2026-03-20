# Consensus Common Library

**Version**: 0.2.0 | [Changelog](CHANGELOG.md)

Shared types, traits, and utilities for TON consensus implementations.

## Overview

The `consensus-common` crate provides the foundation shared by consensus implementations:

- **Catchain-based validator-session**: Original TON consensus
- **Simplex**: Alpenglow-based consensus

### Key Design Decisions

1. **Shared abstractions**: Common overlay interfaces for network communication
2. **Type aliases**: Consistent naming across consensus components
3. **Factory pattern**: Centralized object creation via `ConsensusCommonFactory`
4. **Feature flags**: Optional test utilities via `test-utils` feature

### Relationship to Other Components

```
validator-manager (higher level)
        │
        │ SessionListener callbacks
        ▼
validator-session+catchain / simplex
        │
        │ Consensus types and traits
        ▼
consensus-common ◄── this crate
        │
        │ ConsensusOverlayManager
        ▼
    overlay (lower level, network)
```

## Architecture

### Module Structure

```
┌────────────────────────────────────────────────────────────────────────────────┐
│ consensus-common                                                               │
│                                                                                │
│  ┌─────────────────────────────────────┐  ┌─────────────────────────────────┐  │
│  │ Public Types (lib.rs)               │  │ Public Modules                  │  │
│  │                                     │  │                                 │  │
│  │  - PublicKey, PrivateKey            │  │  - compression (LZ4)            │  │
│  │  - SessionId, BlockHash             │  │  - profiling (check_execution)  │  │
│  │  - ConsensusNode                    │  │  - utils (serialization)        │  │
│  │  - BlockPayload trait               │  │                                 │  │
│  │  - ConsensusOverlay trait           │  │  Feature: test-utils            │  │
│  │  - ConsensusOverlayManager trait    │  │                                 │  │
│  │  - ActivityNode trait               │  │                                 │  │
│  │  - LogPlayer trait                  │  │                                 │  │
│  │  - ConsensusCommonFactory           │  │                                 │  │
│  └─────────────────────────────────────┘  └─────────────────────────────────┘  │
│                                                                                │
│  ┌─────────────────────────────────────────────────────────────────────────┐   │
│  │ Internal Implementations (crate-private)                                │   │
│  │                                                                         │   │
│  │  - activity_node.rs    - Liveness tracking                              │   │
│  │  - block_payload.rs    - BlockPayload implementation                    │   │
│  │  - adnl_overlay.rs     - ADNL-based overlay (production)                │   │
│  │  - in_process_overlay.rs  - In-process overlay (testing)                │   │
│  │  - dummy_catchain_overlay.rs - Dummy overlay (unit tests)               │   │
│  │  - log_player.rs       - Log replay implementation                      │   │
│  │  - async_key_value_storage.rs - Async KV store (RocksDB)                │   │
│  └─────────────────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────────────────┘
```

### Data Flow

```
                    Network                              Application
                        │                                     ▲
                        ▼                                     │
┌─────────────────────────────────────────────────────────────────────────────┐
│ ConsensusOverlay                                                            │
│  - send_message()         - Send to specific validator                      │
│  - send_message_multicast() - Send to multiple validators                   │
│  - send_query()           - Send query with response callback               │
│  - send_broadcast_fec_ex() - Broadcast with FEC                             │
└─────────────────────────────────────────────────────────────────────────────┘
                        │                                     ▲
                        │ ConsensusOverlayListenerPtr         │
                        ▼                                     │
┌─────────────────────────────────────────────────────────────────────────────┐
│ ConsensusOverlayListener                                                    │
│  - on_message()    - Process incoming direct message                        │
│  - on_broadcast()  - Process incoming broadcast                             │
│  - on_query()      - Process incoming query with response callback          │
└─────────────────────────────────────────────────────────────────────────────┘
```

## Key Concepts

### Type Aliases

Common type aliases used throughout the consensus layer:

| Type | Definition | Description |
|------|------------|-------------|
| `PublicKey` | `Arc<dyn KeyOption>` | Validator public key |
| `PublicKeyHash` | `Arc<KeyId>` | Public key hash (ADNL short ID) |
| `PrivateKey` | `Arc<dyn KeyOption>` | Validator private key |
| `SessionId` | `UInt256` | Consensus session identifier |
| `BlockHash` | `UInt256` | Block hash |
| `BlockSignature` | `ton_api::ton::bytes` | Ed25519 signature |
| `RawBuffer` | `ton_api::ton::bytes` | Raw data buffer |
| `ValidatorWeight` | `u64` | Validator voting weight |
| `ValidatorBlockId` | `BlockIdExt` | Block identifier |

### Consensus Node

Describes a participant in the consensus:

```rust
pub struct ConsensusNode {
    pub adnl_id: PublicKeyHash,   // ADNL node short ID
    pub public_key: PublicKey,    // Node public key
}
```

### Overlay Interfaces

| Trait | Direction | Purpose |
|-------|-----------|---------|
| `ConsensusOverlay` | Consensus → Network | Send messages, queries, broadcasts |
| `ConsensusOverlayListener` | Network → Consensus | Receive messages, broadcasts, queries |
| `ConsensusOverlayManager` | Factory | Create and manage overlay instances |
| `ConsensusOverlayLogReplayListener` | Time control | Adjust time during log replay |

### Block Payload

The `BlockPayload` trait defines the interface for consensus block data:

```rust
pub trait BlockPayload: fmt::Debug + Send + Sync {
    fn data(&self) -> &RawBuffer;              // Raw data buffer
    fn get_creation_time(&self) -> SystemTime; // Block creation timestamp
}
```

### Activity Node

For liveness tracking of consensus components:

```rust
pub trait ActivityNode: Send + Sync {
    fn get_name(&self) -> String;                  // Node name
    fn get_creation_time(&self) -> SystemTime;     // Creation timestamp
    fn get_access_time(&self) -> SystemTime;       // Last activity timestamp
    fn tick(&self);                                // Notify activity
}
```

### Log Replay

For debugging and testing with recorded sessions:

| Type | Purpose |
|------|---------|
| `LogReplayOptions` | Configuration (file path, session ID, delays, DB path) |
| `LogPlayer` | Interface for replaying consensus logs |
| `ConsensusReplayListener` | Callbacks for replay start/finish events |

### Async Key-Value Storage

RocksDB-based async storage for consensus persistence (inspired by C++ `td/db/KeyValueAsync.h`):

| Type | Purpose |
|------|---------|
| `AsyncKeyValueStorage` | Trait for async key-value operations |
| `AsyncKeyValueStoragePtr` | `Arc<dyn AsyncKeyValueStorage>` |
| `AsyncKeyValueStorageOptions` | Configuration (use_callback_thread) |
| `StorageKey` | Key type (`Vec<u8>`) |
| `StorageValue` | Value type (`Vec<u8>`) |
| `StorageAsyncResult<T>` | Async result wrapper with `is_ready()`, `try_get()`, `wait()` |
| `StorageWriteCallback` | Optional completion callback for writes |
| `StorageGetCallback` | Optional completion callback for reads |

**Threading Model:**
- **Caller thread**: Posts operations, receives `StorageAsyncResult`
- **DB Processing thread** (`kv-db:{id}`): Processes operations, accesses RocksDB
- **Callback thread** (`kv-callback:{id}`, optional): Executes completion callbacks

**Usage:**

```rust
use consensus_common::{ConsensusCommonFactory, AsyncKeyValueStorageOptions};

// Create storage
let storage = ConsensusCommonFactory::create_async_key_value_storage(
    "/path/to/db",
    "my-session",
    AsyncKeyValueStorageOptions::default(),
)?;

// Fire-and-forget write
storage.set(b"key".to_vec(), b"value".to_vec(), None);

// Async read with blocking wait (with timeout)
let result = storage.get(b"key".to_vec(), None);
let value = result.wait_timeout(Duration::from_secs(5))
    .ok_or_else(|| anyhow::anyhow!("timeout"))??;

// Or wait indefinitely
let value = result.wait()?;

// Sync all pending operations
storage.sync(Some(Duration::from_secs(10)))?;

// Mark for destruction on drop
storage.mark_for_destroy();
```

## Package Structure

```
node/consensus-common/
├── Cargo.toml                 # Package manifest
├── README.md                  # This file
├── CHANGELOG.md               # Version history
└── src/
    ├── lib.rs                 # Public API: types, traits, factory
    ├── activity_node.rs       # ActivityNode implementation (crate-private)
    ├── adnl_overlay.rs        # ADNL overlay manager (crate-private)
    ├── async_key_value_storage.rs # Async KV storage (crate-private impl)
    ├── block_payload.rs       # BlockPayload implementation (crate-private)
    ├── compression.rs         # LZ4 compression for block candidates (public)
    ├── dummy_catchain_overlay.rs  # Dummy overlay for tests (crate-private)
    ├── in_process_overlay.rs  # In-process overlay for tests (crate-private)
    ├── log_player.rs          # Log replay implementation (crate-private)
    ├── node_test_network.rs   # Test network utilities (feature: test-utils)
    ├── profiling.rs           # Execution time profiling (public)
    ├── utils.rs               # Serialization utilities (public)
    └── tests/
        └── test_async_key_value_storage.rs  # 22 unit tests
```

## Components

### Public API (`lib.rs`)

Entry point for all consensus implementations.

| Type | Purpose |
|------|---------|
| `ConsensusNode` | Validator node description |
| `SessionStats` | Session statistics for committed blocks |
| `BlockPayload` | Trait for block payload data |
| `BlockPayloadPtr` | `Arc<dyn BlockPayload>` |
| `ConsensusOverlay` | Outgoing overlay interface (trait) |
| `ConsensusOverlayListener` | Inbound overlay interface (trait) |
| `ConsensusOverlayManager` | Overlay factory interface (trait) |
| `ActivityNode` | Liveness tracking interface (trait) |
| `LogPlayer` | Log replay interface (trait) |
| `LogReplayOptions` | Log replay configuration |
| `AsyncKeyValueStorage` | Async key-value storage interface (trait) |
| `AsyncKeyValueStoragePtr` | `Arc<dyn AsyncKeyValueStorage>` |
| `AsyncKeyValueStorageOptions` | Storage configuration |
| `StorageAsyncResult<T>` | Async result wrapper (trait) |
| `ConsensusCommonFactory` | Factory for creating shared objects |

### ConsensusCommonFactory

Factory methods for creating shared objects:

| Method | Returns | Description |
|--------|---------|-------------|
| `create_block_payload(data)` | `BlockPayloadPtr` | Create block payload |
| `create_empty_block_payload()` | `BlockPayloadPtr` | Create empty payload |
| `create_activity_node(name)` | `ActivityNodePtr` | Create activity tracker |
| `create_dummy_overlay_manager()` | `ConsensusOverlayManagerPtr` | Dummy overlay (unit tests) |
| `create_in_process_overlay_manager(threads)` | `ConsensusOverlayManagerPtr` | In-process overlay (integration tests) |
| `create_adnl_overlay_manager(...)` | `Result<ConsensusOverlayManagerPtr>` | ADNL overlay (production) |
| `create_log_player(options)` | `Result<LogPlayerPtr>` | Create log player |
| `create_log_players(options)` | `Vec<LogPlayerPtr>` | Enumerate all sessions in log |
| `create_async_key_value_storage(path, id, opts)` | `Result<AsyncKeyValueStoragePtr>` | Create async KV storage |

### Compression Module (`compression.rs`)

LZ4 compression utilities for block candidates:

| Function | Purpose |
|----------|---------|
| `compress_candidate_data(block, collated_data)` | Compress block + collated data into single LZ4 buffer |
| `decompress_candidate_data(compressed, size)` | Decompress back to block and collated data |

**Usage:**

```rust
use consensus_common::compression::{compress_candidate_data, decompress_candidate_data};

// Compress block candidate
let (compressed, decompressed_size) = compress_candidate_data(&block_data, &collated_data)?;

// Decompress block candidate
let (block_data, collated_data) = decompress_candidate_data(&compressed, decompressed_size)?;
```

### Profiling Module (`profiling.rs`)

Execution time tracking utilities:

| Macro | Purpose |
|-------|---------|
| `check_execution_time!` | Measure execution time of code blocks |
| `instrument!` | Instrument functions for profiling |

### Utils Module (`utils.rs`)

Serialization utilities:

| Macro | Purpose |
|-------|---------|
| `serialize_tl_bare_object!` | Serialize TL bare object |
| `serialize_tl_boxed_object!` | Serialize TL boxed object |

## Configuration

### Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `telemetry` | ✅ | Enable telemetry via ADNL and storage |
| `export_key` | ❌ | Enable key export functionality |
| `test-utils` | ❌ | Enable test utilities (`node_test_network` module) |

## Integration

### Using ConsensusCommonFactory

```rust
use consensus_common::{
    ConsensusCommonFactory, ConsensusNode, LogReplayOptions,
    ConsensusOverlayManagerPtr, BlockPayloadPtr,
};

// Create block payload
let payload = ConsensusCommonFactory::create_block_payload(data);

// Create activity tracker
let activity = ConsensusCommonFactory::create_activity_node("my-node".into());
activity.tick(); // Notify activity

// Create overlay manager for testing
let overlay = ConsensusCommonFactory::create_in_process_overlay_manager(4);

// Create overlay manager for production
let overlay = ConsensusCommonFactory::create_adnl_overlay_manager(
    runtime.handle().clone(),
    network_stack,
    Some(3),  // broadcast_hops
    true,     // track_private_peers
)?;

// Log replay
let options = LogReplayOptions {
    log_file_name: "session.log".into(),
    session_id: None,  // Use last session
    replay_without_delays: true,
    db_path: "/tmp/db".into(),
    db_suffix: "test".into(),
    allow_unsafe_self_blocks_resync: false,
};
let player = ConsensusCommonFactory::create_log_player(&options)?;
```

### Implementing ConsensusOverlayListener

```rust
use consensus_common::{
    ConsensusOverlayListener, PublicKeyHash, BlockPayloadPtr, QueryResponseCallback,
};

struct MyListener;

impl ConsensusOverlayListener for MyListener {
    fn on_message(&self, adnl_id: PublicKeyHash, data: &BlockPayloadPtr) {
        // Process incoming direct message
    }

    fn on_broadcast(&self, source_key_hash: PublicKeyHash, data: &BlockPayloadPtr) {
        // Process incoming broadcast
    }

    fn on_query(
        &self,
        adnl_id: PublicKeyHash,
        data: &BlockPayloadPtr,
        response_callback: QueryResponseCallback,
    ) {
        // Process query and call response_callback with result
        let response = ConsensusCommonFactory::create_block_payload(result);
        response_callback(Ok(response));
    }
}
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| `adnl` | Network layer, overlay management |
| `ton_block` | Block types, key types |
| `ton_api` | TL serialization |
| `storage` | Database access, RocksDB wrapper |
| `rocksdb` | Key-value storage backend |
| `tokio` | Async runtime |
| `crossbeam` | Concurrent data structures |

### Optional Dependencies (test-utils feature)

| Crate | Purpose |
|-------|---------|
| `external-ip` | IP address detection for tests |
| `serde_json` | JSON configuration for tests |

## License

See the [LICENSE](../../LICENSE) file for details.
