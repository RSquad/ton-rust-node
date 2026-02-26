# Changelog

All notable changes to the TON Rust Node will be documented in this file.
For Helm chart changes, see [helm/ton-rust-node/CHANGELOG.md](helm/ton-rust-node/CHANGELOG.md).

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the node release tags (e.g. `v0.1.2-mainnet`).

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
