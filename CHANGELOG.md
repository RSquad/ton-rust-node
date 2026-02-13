# Changelog

All notable changes to the TON Rust Node will be documented in this file.
For Helm chart changes, see [helm/ton-rust-node/CHANGELOG.md](helm/ton-rust-node/CHANGELOG.md).

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the node release tags (e.g. `v0.1.2-mainnet`).

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
