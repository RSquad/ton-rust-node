# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the TON Rust Node release tags (e.g. `v0.1.2-mainnet`).

## [v0.1.2-mainnet] - 2026-02-12

Image: `ghcr.io/rsquad/ton-rust-node:v0.1.2-mainnet`
Chart: `0.2.2`

### Fixed

- Metrics port is no longer exposed on public LoadBalancer services. A dedicated `<release>-metrics` ClusterIP service is created instead for internal scraping via ServiceMonitor or annotations. See [docs/monitoring.md](helm/ton-rust-node/docs/monitoring.md) for details.

## [v0.1.2-mainnet] - 2026-02-11

Image: `ghcr.io/rsquad/ton-rust-node:v0.1.2-mainnet`
Chart: `0.2.0`

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
- Grafana dashboard as TypeScript (Foundation SDK) in `grafana/`

### Changed

- Helm chart: image tag pinned to specific version (no more floating `mainnet` tag)
- Helm chart: init container image parameterized via `initImage.*`
- Helm chart: `pullPolicy` default changed from `Always` to `IfNotPresent`

### Removed

- StatsD metrics exporter
- Legacy feature flags (`prometheus`, `log_metrics`)

### Fixed

- SAMEALTSAVE mnemonic
- Missing MC block metrics by emitting from `save_last_applied_mc_block_id`
- RLDP addresses cache with bad peers tracking

## [v0.1.0-mainnet] - 2026-02-04

Image: `ghcr.io/rsquad/ton-rust-node:v0.1.0-mainnet`
Chart: `0.1.0`

Initial release.
