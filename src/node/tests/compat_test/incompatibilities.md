# Rust ↔ C++ Compatibility Test Results

Cross-implementation compatibility testing between the Rust (`adnl` crate) and C++ (`ton-cpp-testnet`) overlay/ADNL implementations.

## Known Issues

### Safe/RLDP Overlay Message Delivery (BROKEN)

Overlay messages sent via Safe/RLDP transport (TCP-like) from Rust to C++ are not delivered. RLDP queries work fine in both directions; only fire-and-forget `overlay.message()` via RLDP is affected.

- **Test**: `test_overlay_message::test_overlay_message_rust_to_cpp_safe` (ignored)
- **Workaround**: Use Fast/UDP for overlay messages (works correctly)

## Test Summary

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
| **Total** | **53** | **52** | **1** | |

## What Is Tested

- **Overlay ID computation** — identical IDs from same inputs (ASCII, binary, Unicode)
- **Broadcasts** — small (inline) and FEC-encoded (2KB), both directions
- **Broadcast validation** — 2-phase accept/reject callback
- **Overlay queries** — echo roundtrip and rejection behavior
- **Overlay messages** — point-to-point delivery, burst (20 msgs, ≥90% required)
- **BOC compression** — bidirectional, 2 algorithms, multiple cell topologies, round-trip
- **Candidate ID signing** — TL serialization byte match
- **RLDP v1/v2** — query/response at 256B, 4KB, 7KB payloads, both directions
- **FEC relay** — 3-node redistribution, all 4 Rust/C++ role combinations
- **TwostepFec relay** — 6-node redistribution with mixed Rust/C++ bridges
- **QUIC transport** — TLS/RPK handshake, raw queries, large messages (900B)
- **QUIC overlay** — overlay messages and queries routed via QUIC
- **QUIC private overlay** — ADNL vs QUIC transport, burst delivery (100% required)

## Reproduction

```bash
export CPP_SRC_PATH=/path/to/ton-cpp-testnet

# Run all tests
make test

# Run a specific test suite
make test TEST=test_broadcast

# Run with verbose output
RUST_LOG=debug make test TEST=test_rldp_query
```
