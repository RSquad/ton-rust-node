# Cross-Implementation Compatibility Tests

This directory contains tests that verify compatibility between the Rust ADNL/overlay implementation and the C++ reference implementation.

## Prerequisites

1. **C++ TON source code**: You need access to the C++ TON node source code (ton-cpp-testnet)
2. **Pre-built C++ libraries**: The C++ TON libraries must be built before running tests
3. **C++ build dependencies**: CMake, C++ compiler (with C++20 support), OpenSSL, ZLIB, etc.
4. **Rust toolchain**: cargo, rustc

## Directory Structure

```
compat_test/
├── README.md              # This file
├── Cargo.toml             # Rust package manifest
├── Makefile               # Build and test automation
├── incompatibilities.md   # Detailed compatibility report and found bugs
├── cpp_src/               # C++ test harness source code
│   ├── CMakeLists.txt
│   ├── compat_test_node.cpp
│   └── compat_test_node.hpp
├── src/                   # Rust library code
│   ├── lib.rs             # CppTestNode wrapper and JSON protocol
│   ├── overlay_id.rs      # Overlay ID computation helpers
│   └── test_helpers.rs    # RustTestNode, RustQuicTestNode, and test utilities
├── tests/                 # Rust integration tests
│   ├── test_overlay_id.rs              # Overlay ID computation compatibility
│   ├── test_broadcast.rs              # Broadcast delivery (small + FEC, both directions)
│   ├── test_broadcast_validation.rs   # 2-phase broadcast accept/reject callbacks
│   ├── test_public_overlay.rs         # Overlay query/response compatibility
│   ├── test_overlay_message.rs        # Point-to-point overlay messages
│   ├── test_boc_compression.rs        # BOC compression interoperability
│   ├── test_candidate_id_to_sign.rs   # Consensus candidate ID TL serialization
│   ├── test_rldp_query.rs            # RLDP v1/v2 query/response (multiple sizes)
│   ├── test_fec_relay.rs             # FEC broadcast relay (3-node topology)
│   ├── test_twostep_fec_relay.rs     # TwostepFec broadcast relay (6-node topology)
│   ├── test_quic_transport.rs        # QUIC transport: raw queries, large messages, TLS
│   ├── test_quic_overlay.rs          # QUIC overlay: messages and queries via QUIC
│   ├── test_quic_private_overlay.rs  # QUIC private overlay: ADNL vs QUIC transport
│   └── test_raptorq.rs              # RaptorQ FEC codec cross-implementation tests
└── build/                 # Build artifacts (gitignored)
    └── cpp/               # C++ binary output
```

## Usage

### Building C++ TON Libraries (One-time Setup)

Before running tests, you must build the C++ TON libraries:

```bash
cd /path/to/ton-cpp-testnet
mkdir -p build && cd build
cmake ..
cmake --build . --target overlay adnl dht tl_api keys keyring fec rldp rldp2 tdutils tdactor tdnet ton_crypto
```

### Running All Tests

```bash
CPP_SRC_PATH=/path/to/ton-cpp-testnet make test
```

### Running Specific Test Suite

```bash
CPP_SRC_PATH=/path/to/ton-cpp-testnet make test TEST=test_broadcast
```

### Building Only

```bash
CPP_SRC_PATH=/path/to/ton-cpp-testnet make build
```

### Cleaning Build Artifacts

```bash
make clean
```

## Compatibility Status

| Test Suite | Tests | Pass | Ignored | Status |
|------------|-------|------|---------|--------|
| `test_overlay_id` | 4 | 4 | 0 | Compatible |
| `test_broadcast` | 4 | 4 | 0 | Compatible |
| `test_broadcast_validation` | 4 | 4 | 0 | Compatible |
| `test_public_overlay` | 2 | 2 | 0 | Compatible |
| `test_overlay_message` | 5 | 4 | 1 | 1 ignored (Safe/RLDP) |
| `test_boc_compression` | 4 | 4 | 0 | Compatible |
| `test_candidate_id_to_sign` | 2 | 2 | 0 | Compatible |
| `test_rldp_query` | 8 | 8 | 0 | Compatible |
| `test_fec_relay` | 4 | 4 | 0 | Compatible |
| `test_twostep_fec_relay` | 4 | 4 | 0 | Compatible |
| `test_quic_transport` | 3 | 3 | 0 | Compatible |
| `test_quic_overlay` | 4 | 4 | 0 | Compatible |
| `test_quic_private_overlay` | 5 | 5 | 0 | Compatible |
| `test_raptorq` | 14 | 14 | 0 | Compatible |
| **Total** | **67** | **66** | **1** | |

## Test Suites

### 1. Overlay ID (`test_overlay_id`)
- Rust and C++ compute identical overlay short IDs for various name formats (ASCII, binary, Unicode)
- C++ harness infrastructure checks (ping, ADNL ID)

### 2. Broadcasts (`test_broadcast`)
Small (inline) and FEC-encoded (2KB) broadcasts in both directions:

| Test | Direction | Payload | Result |
|------|-----------|---------|--------|
| `test_broadcast_rust_to_cpp` | Rust → C++ | small (26 B) | PASS |
| `test_broadcast_cpp_to_rust` | C++ → Rust | small (25 B) | PASS |
| `test_fec_broadcast_rust_to_cpp` | Rust → C++ | FEC (2 KB) | PASS |
| `test_fec_broadcast_cpp_to_rust` | C++ → Rust | FEC (2 KB) | PASS |

### 3. Broadcast Validation (`test_broadcast_validation`)
- 2-phase `check_broadcast` accept/reject callback (Rust sender → C++ receiver)
- Accept mode: broadcast delivered to application layer
- Reject mode: broadcast dropped, not delivered
- Validator mode toggling

### 4. Overlay Queries (`test_public_overlay`)
- Query/response echo roundtrip (C++ → Rust)
- Query rejection behavior (Rust → C++, expects timeout — C++ drops rejected queries silently)

### 5. Overlay Messages (`test_overlay_message`)
Point-to-point overlay messages (the same path used by simplex consensus for votes and certificates):

| Test | Direction | What | Result |
|------|-----------|------|--------|
| `test_overlay_message_cpp_to_rust` | C++ → Rust | Single message, receipt verified | PASS |
| `test_overlay_message_rust_to_cpp` | Rust → C++ | Single message via Fast/UDP | PASS |
| `test_overlay_message_rust_to_cpp_safe` | Rust → C++ | Single message via Safe/RLDP | IGNORED |
| `test_overlay_message_burst_rust_to_cpp` | Rust → C++ | 20 messages, ≥90% delivery | PASS |
| `test_overlay_message_cpp_to_cpp_baseline` | C++ ↔ C++ | Baseline (no Rust) | PASS |

### 6. BOC Compression (`test_boc_compression`)
- Bidirectional compress/decompress with `BaselineLZ4` and `ImprovedStructureLZ4` algorithms
- Three cell topologies: single cell, tree with shared refs (DAG), simple tree
- Full round-trip (Rust compress → C++ decompress → C++ compress → Rust decompress)
- Multi-root BOC in both directions

### 7. Candidate ID Signing (`test_candidate_id_to_sign`)
- TL serialization byte match for `consensus.candidateId` across 4 (slot, hash) combos
- Negative check: verifies C++ signs `candidateId` directly, not `candidateParent` wrapper

### 8. RLDP Query/Response (`test_rldp_query`)
Both RLDP v1 and v2, both directions, three payload sizes:

| Test | Sender | Responder | RLDP | Payload | Result |
|------|--------|-----------|------|---------|--------|
| `test_rldp_v1_rust_to_cpp` | Rust | C++ | v1 | 256 B | PASS |
| `test_rldp_v1_cpp_to_rust` | C++ | Rust | v1 | 256 B | PASS |
| `test_rldp_v2_rust_to_cpp` | Rust | C++ | v2 | 256 B | PASS |
| `test_rldp_v2_cpp_to_rust` | C++ | Rust | v2 | 256 B | PASS |
| `test_rldp_v2_4kb_rust_to_cpp` | Rust | C++ | v2 | 4 KB | PASS |
| `test_rldp_v2_4kb_cpp_to_rust` | C++ | Rust | v2 | 4 KB | PASS |
| `test_rldp_v2_7kb_rust_to_cpp` | Rust | C++ | v2 | 7 KB | PASS |
| `test_rldp_v2_7kb_cpp_to_rust` | C++ | Rust | v2 | 7 KB | PASS |

**Note**: Query data must be a valid TL-serialized object (not raw bytes) because Rust's `deserialize_boxed_bundle` requires it.

**Payload size constraints**:
- RLDP v1 `default_mtu` = 1024 bytes — limits unsolicited incoming transfers to ~928 bytes of user data
- RLDP v2 `DEFAULT_MTU` = 7680 bytes — allows up to ~7.5 KB without configuration
- C++ overlay `huge_packet_max_size()` = 8192 bytes — hard limit on query data before RLDP wrapping

### 9. FEC Relay (`test_fec_relay`)
3-node linear topology (Sender → Relay → Receiver), sender and receiver NOT directly connected. Broadcast size: 2000 bytes (triggers FEC encoding at >768 bytes):

| Test | Sender | Relay | Receiver | Result |
|------|--------|-------|----------|--------|
| `test_fec_relay_rust_cpp_rust` | Rust | C++ | Rust | PASS |
| `test_fec_relay_cpp_rust_cpp` | C++ | Rust | C++ | PASS |
| `test_fec_relay_rust_rust_cpp` | Rust | Rust | C++ | PASS |
| `test_fec_relay_cpp_cpp_rust` | C++ | C++ | Rust | PASS |

### 10. TwostepFec Relay (`test_twostep_fec_relay`)
6-node topology (Sender → 4 Bridges → Leaf), leaf NOT directly connected to sender. Broadcast size: 2048 bytes (>= 513 bytes triggers TwostepFec with FEC encoding):

| Test | Sender | Bridges | Leaf | Result |
|------|--------|---------|------|--------|
| `test_twostep_rust_sender_cpp_leaf` | Rust | 4 Rust | C++ | PASS |
| `test_twostep_cpp_sender_rust_leaf` | C++ | 4 C++ | Rust | PASS |
| `test_twostep_mixed_bridges_rust_leaf` | Rust | 2 Rust + 2 C++ | Rust | PASS |
| `test_twostep_mixed_bridges_cpp_leaf` | C++ | 2 Rust + 2 C++ | C++ | PASS |

### 11. QUIC Transport (`test_quic_transport`)
- Raw QUIC query echo (C++ → Rust) with TL-serialized payload
- Large overlay message via QUIC (900B, near C++ 1024-byte per-stream limit)
- QUIC connection establishment — TLS handshake with RPK (Raw Public Key) certificates

### 12. QUIC Overlay (`test_quic_overlay`)
- C++ ↔ C++ QUIC overlay message baseline
- Overlay message via QUIC (Rust → C++, with UDP baseline comparison)
- Raw QUIC message delivery (C++ → Rust)
- Overlay query via QUIC (Rust → C++) with echo handler

### 13. QUIC Private Overlay (`test_quic_private_overlay`)
- Private overlay message via ADNL (baseline)
- Private overlay message via QUIC transport (Rust → C++)
- QUIC message burst (20 messages, 100% delivery required — stream-based, no UDP loss)
- QUIC overlay query (Rust → C++)
- Private overlay message (C++ → Rust, with receipt verification)

### 14. RaptorQ FEC Codec (`test_raptorq`)
Cross-implementation RaptorQ encode/decode — symbols produced by one side are fed to the other's decoder. No networking involved; the C++ test node exposes `raptorq_encode`/`raptorq_decode` commands that operate on raw data:

| Test | Direction | Payload | Scenario | Result |
|------|-----------|---------|----------|--------|
| `test_rust_encode_cpp_decode_small` | Rust → C++ | 500 B | Source-only | PASS |
| `test_rust_encode_cpp_decode_medium` | Rust → C++ | 10 KB | With repair symbols | PASS |
| `test_rust_encode_cpp_decode_large` | Rust → C++ | 100 KB | Large payload | PASS |
| `test_cpp_encode_rust_decode_small` | C++ → Rust | 500 B | Source-only | PASS |
| `test_cpp_encode_rust_decode_medium` | C++ → Rust | 10 KB | With repair symbols | PASS |
| `test_cpp_encode_rust_decode_large` | C++ → Rust | 100 KB | Large payload | PASS |
| `test_rust_encode_cpp_decode_with_loss` | Rust → C++ | 10 KB | 2 source symbols dropped, repaired | PASS |
| `test_cpp_encode_rust_decode_with_loss` | C++ → Rust | 10 KB | 2 source symbols dropped, repaired | PASS |
| `test_params_match` | Both | 9 sizes | Parameters identical (100B–100KB) | PASS |
| `test_source_symbols_identical` | Both | 3 sizes | Source symbols byte-identical | PASS |
| `test_4mb_rust_encode_cpp_decode` | Rust → C++ | 4 MB | 4/8/16/32/64 symbols (>=64KB each), SHA-256 verified | PASS |
| `test_4mb_cpp_encode_rust_decode` | C++ → Rust | 4 MB | 4/8/16/32/64 symbols (>=64KB each), SHA-256 verified | PASS |
| `test_4mb_large_symbol_params_match` | Both | 4 MB | Parameters identical for all 5 symbol counts | PASS |
| `test_4mb_large_symbol_source_identical` | Both | 4 MB | Source symbols byte-identical for all 5 symbol counts | PASS |

## Environment Variables

| Variable | Description | Required |
|----------|-------------|----------|
| `CPP_SRC_PATH` | Path to C++ TON source (ton-cpp-testnet) | Yes |
| `CPP_BUILD_DIR` | Path for C++ test binary build (default: `./build/cpp`) | No |
| `CMAKE_BUILD_TYPE` | CMake build type (default: `Release`) | No |
| `TEST` | Specific test suite name filter | No |
| `RUST_LOG` | Rust logging level (e.g., `debug`, `trace`) | No |
| `RUST_TEST_THREADS` | Number of test threads (default: `1` for serial execution) | No |

## C++ Test Node Protocol

The C++ test node (`compat_test_node`) communicates via JSON over stdin/stdout:

```json
// Commands:
{"cmd": "ping"}
{"cmd": "add_peer", "pubkey": "BASE64_TL_PUBKEY", "ip": "127.0.0.1", "port": 14001}
{"cmd": "create_overlay", "type": "private", "overlay_name": "BASE64", "peers": ["ADNL_ID_HEX"]}
{"cmd": "send_broadcast", "overlay_id": "HEX", "data": "BASE64", "use_fec": false}
{"cmd": "send_message", "overlay_id": "HEX", "peer_adnl_id": "HEX", "data": "BASE64"}
{"cmd": "set_broadcast_validator", "overlay_id": "HEX", "mode": "accept_all|reject_all"}
{"cmd": "set_query_handler", "overlay_id": "HEX", "mode": "echo|reject"}
{"cmd": "get_received_broadcasts", "overlay_id": "HEX"}
{"cmd": "get_received_messages", "overlay_id": "HEX"}
{"cmd": "send_query", "overlay_id": "HEX", "peer_adnl_id": "HEX", "data": "BASE64", "timeout_ms": 5000}
{"cmd": "send_rldp_query", "overlay_id": "HEX", "peer_adnl_id": "HEX", "data": "BASE64", "max_answer_size": 1048576, "v2": true}
{"cmd": "enable_quic"}
{"cmd": "send_quic_message", "peer_adnl_id": "HEX", "data": "BASE64"}
{"cmd": "send_quic_query", "peer_adnl_id": "HEX", "data": "BASE64", "timeout_ms": 5000}
{"cmd": "raptorq_encode", "data": "BASE64", "symbol_size": 768, "repair_count": 2}
{"cmd": "raptorq_decode", "data_size": 10000, "symbol_size": 768, "symbols_count": 14, "symbols": [{"id": 0, "data": "BASE64"}, ...]}
{"cmd": "shutdown"}

// Responses:
{"result": ...}
{"error": "..."}
```

## Port Ranges

Tests use different port ranges to avoid conflicts:
- `test_overlay_id`: 14010-14019
- `test_broadcast`: 15100-15149
- `test_public_overlay`: 15150-15199
- `test_broadcast_validation`: 15300-15399
- `test_overlay_message`: 15400-15499
- `test_boc_compression`: 15500-15599
- `test_fec_relay`: 15600-15699
- `test_twostep_fec_relay`: 15700-15799
- `test_rldp_query`: 15800-15899
- `test_candidate_id_to_sign`: 15900-15909
- `test_quic_transport`: 18000-18099
- `test_quic_overlay`: 18100-18199
- `test_quic_private_overlay`: 18200-18299
- `test_raptorq`: 19000-19099

## Troubleshooting

### C++ Build Fails
- Ensure C++ TON libraries are pre-built in `$CPP_SRC_PATH/build`
- Check that CMake can find OpenSSL and ZLIB
- Verify C++20 compiler support

### Tests Timeout
- Ensure no firewall blocks UDP ports 14000-20000 on localhost
- Check that no other processes use the same ports
- Try increasing sleep durations in tests if running on slow hardware

### "broadcast source certificate is invalid"
This error in C++ logs indicates the overlay privacy rules are too restrictive. The test node should use `AllowFec` flag without `Trusted` to enable 2-phase validation.

### Overlay ID Mismatch
If Rust and C++ compute different overlay IDs:
- Verify the overlay name bytes are identical (check base64 encoding)
- Ensure both use the TL `pub.overlay{name}` wrapper before hashing
