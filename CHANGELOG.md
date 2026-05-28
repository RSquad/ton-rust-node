# Changelog

All notable changes to the TON Rust Node will be documented in this file.
For Helm chart changes, see [helm/ton-rust-node/CHANGELOG.md](helm/ton-rust-node/CHANGELOG.md).

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the node release tags (e.g. `v0.1.2-mainnet`).

## [v0.8.0] - 2026-05-26

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.8.0`

### Changed

- Cell DB performance boost via RocksDB merge operator

### Fixed

- Stability updates

## [v0.7.1] - 2026-05-24

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.7.1`

### Added

- HTTP SSE endpoint for confirmed blocks

### Fixed

- Secure vault operation

## [v0.7.0] - 2026-05-21

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.7.0`

### Added

- Do not expose node keys in config, use secrets vault

### Changed

- External messages processing optimization
- Memory optimization for archival node
- Performance and memory optimizations for CellDb
- Stability improvements for compressed BOCs and external messages
- TVM stability updates

### Fixed

- Fixes for protocol vulnerabilities

## [v0.6.1] - 2026-05-06

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.6.1`

### Added

- CLI tool for copying node secrets to HashiCorp Vault

### Changed

- ADNL key management for fast sync overlay

### Fixed

- Ensure deterministic gas charging in TVM for partial-bit opcodes across all validators
- Align ActionPhase TL-B encoding in transaction executor with the consensus-observable form (post-BOC round-trip)

## [v0.6.0] - 2026-05-01

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.6.0`

### Added

- Support of HashiCorp vault for node secrets storing

### Changed

- Improved performance of operations with cell data representation
- Improved performance and stability of collator
- Improved performance of Simplex consensus protocols
- Improved performance of node cells database

### Fixed

- Compatibility issues for transaction executor and TVM
- Fix for anycast addresses in old transactions
- Proper archive truncation after node hard reboots

## [v0.5.3] - 2026-04-23

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.5.3`

### Changed

- Optimize QUIC for high latency links
- Keep permanent ADNL validator keys

## [v0.5.2] - 2026-04-21

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.5.2`

### Changed

- Optimized fast-sync overlay operation over QUIC

### Fixed

- TVM stack slice TL-serialization fix for LiteServer

## [v0.5.1] - 2026-04-17

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.5.1`

### Added

- Switch to ubuntu base image and add console binary

### Fixed

- Fix LiteServer response handling for listBlockTransactions so clients accept it correctly

## [v0.5.0] - 2026-04-11

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.5.0`

### Added

- Support non-standard RaptorQ symbol size (> 65536)
- Simplex consensus updates (pipeline context, latest protocol changes)
- QUIC connection deduplication

### Changed

- LiteServer runSmcMethod implementation
- Optimized collection of overlay statistics

### Fixed

- Stability fix for fast Merkle update apply
- Transaction execution fixes (two rounds)
- Fix pipeline context for Simplex consensus
- Fix config params creation for emulator

## [v0.4.0] - 2026-04-05

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.4.0`

This release brings support for the Simplex consensus protocol and QUIC transport — key protocol upgrades rolling out across the TON network. It also introduces archival node functionality and a range of fixes to fee accounting, storage phase handling, and sync stability.

### Added

- Simplex consensus updates and QUIC integration
- QUIC transport with separate address support and connection deduplication
- Archival node functionality with split/merge resilience
- CellsDB cells cache

### Changed

- Validate query uses capabilities from blockchain config instead of candidate block
- Enforce mcStateExtra flags <=1, remove ValidatorsStat

### Fixed

- Storage phase: preserve original due_payment for special accounts in partial storage phase
- Masterchain ValueFlow burned fees and blackhole accounting
- Fee accumulation: accumulate fees_collected instead of overwriting to preserve shard burn
- Untouched account change detection
- Fast sync overlay creation reliability
- Archive sync stalling on shard split/merge

## [v0.3.0] - 2026-03-12

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.3.0`

### Added

- Simplex consensus implementation (feature-gated)
- JSON-RPC: `getAccount` and `getBlock` methods, updated OpenAPI spec
- State downloads limit
- Applied blocks metric
- TPS measurement tool (test tooling)

### Changed

- ADNL broadcast improvements: randomized neighbour selection, reduced wave size, FEC timeout tuning, wider GetRandomPeers spreading
- ADNL send buffer limits increased
- Telemetry: switch to `current_average` for throughput calculations
- Disabled validation/collation task await for non-accelerated consensus

### Fixed

- Slot bounds: receiver-level checks before signature verification, sanity checks to prevent unbounded window allocation
- Diagnostic dump no longer lists self in inactive nodes
- LDX primitives: fix load 0 bits
- Base gas check before execute to prevent out-of-gas on commit
- Message stat calculation and storage stat update after balance change
- Collation error handling: reset `is_collating` on error, cross-check notarize/finalize hashes
- Earliest collation time handling for simplex consensus
- Session restart: preserve DB on stop, register overlay before bootstrap, replace dead overlay clients
- ADNL packet parse and multipart handling
- Two-step broadcast ID validation
- `LITESERVER_PUBLIC_KEY` parsing
- Overlay listener leak on failed `start_overlay`
- FinalCert: broadcast only on local creation, not on external ingest

## [v0.2.1-mainnet] - 2026-02-27

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.2.1-mainnet`

### Fixed

- Secrets vault: backward compatibility for vault files created by v0.1.x (path separator changed from `/` to `.`)

## [v0.2.0-mainnet] - 2026-02-27

Image: `ghcr.io/rsquad/ton-rust-node/node:v0.2.0-mainnet`

### Added

- Deferred messages in collator with dispatch queues, per-account processing, and configurable limits
- TVM emulator with C FFI for transaction emulation and `runGetMethod`
- Stabilized Liteserver responses on fresh blocks
- CellsDB: bugfixes and performance improvements
- Merkle update speedup via custom cells DB loader (`apply_for_with_cells_loader`)
- Control server: anonymous client access (no explicit authorization required)

### Changed

- Container image moved from `ghcr.io/rsquad/ton-rust-node` to `ghcr.io/rsquad/ton-rust-node/node`
- Vault config removed from node config JSON — connection now configured via environment variables only
- `StorageCell` renamed to `StoredCell`
- JSON-RPC: `sendBoc` payload limit removed

### Fixed

- `GLOBALID` TVM primitive — incorrect handling of negative integers (e.g. `-239`)
- Storage limits off-by-one rejecting cells/bits exactly at the limit
- Validator session crash from `Duration` overflow
- Several small fixes in VM types JSON import/export

## [v0.1.2-mainnet] - 2026-02-11

Image: `ghcr.io/rsquad/ton-rust-node:v0.1.2-mainnet`

### Added

- Prometheus metrics
- Liveness and readiness probes on metrics HTTP server (`/healthz`, `/readyz`)
- Metrics config moved from env vars to `config.json`
- Liteserver LRU cache
- Liteserver fast/slow pipeline split for light vs heavy queries
- Shared wait registry for `waitMasterchainSeqno`
- New JSON-RPC methods: `getBlockBoc`
- Key-block mode for `getConfigParams` with zerostate fallback
- Overlay ping, two-step simple broadcasts

### Removed

- StatsD metrics exporter
- Legacy feature flags (`prometheus`, `log_metrics`)

### Fixed

- SAMEALTSAVE mnemonic
- Missing MC block metrics by emitting from `save_last_applied_mc_block_id`
- RLDP addresses cache with bad peers tracking

## [v0.1.0-mainnet] - 2026-02-04

Image: `ghcr.io/rsquad/ton-rust-node:v0.1.0-mainnet`

Initial release.
