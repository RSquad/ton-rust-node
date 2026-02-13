# Changelog

All notable changes to the Helm chart will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the Helm chart release tags (e.g. `helm/v0.3.0`).

## [0.3.0] - 2026-02-13

appVersion: `v0.1.2-mainnet`

Huge thanks to [Kiln](https://www.kiln.fi/) ([@kilnfi](https://github.com/kilnfi)) for the detailed feedback that shaped this release. You are the best <3

### Added

- `extraInitContainers` — inject custom init containers before the node starts
- `extraContainers` — run sidecar containers alongside the node
- `extraVolumes` and `extraVolumeMounts` — mount additional volumes into the pod and main container
- `podAnnotations` and `podLabels` — custom pod metadata for Vault injection, service mesh, cost tracking, etc.
- `extraEnv` — environment variables for the main container (Downward API, ConfigMap/Secret refs)
- `extraEnvFrom` — inject all keys from a Secret or ConfigMap as environment variables
- `serviceAccount` — create and bind a dedicated ServiceAccount (for Vault auth, cloud IAM)
- `networkPolicy` — optional NetworkPolicy with public ADNL ingress and configurable TCP CIDRs

### Changed

- **BREAKING:** `services` restructured to per-port configuration. Each port (adnl, control, liteserver, jsonRpc) gets its own Service with independent type, annotations, and perReplica overrides. Migration: `services.type` → `services.adnl.type`, `services.perReplica` → `services.adnl.perReplica`, etc.
- **BREAKING:** `hostPort.enabled` replaced with per-port booleans: `hostPort.adnl`, `hostPort.control`, `hostPort.liteserver`, `hostPort.jsonRpc`. Migration: `hostPort.enabled: true` → `hostPort.adnl: true`.
- Default service type for liteserver and jsonRpc changed to LoadBalancer (was implicitly LoadBalancer via shared `services.type`, now explicit per-port)
- `serviceAccount.create` renamed to `serviceAccount.enabled` for consistency with other feature toggles

### Fixed

- Control port defaults to ClusterIP instead of sharing the LoadBalancer with other ports
- Missing logger targets in documentation (`overlay_broadcast`, `adnl_query`, `validate_reject`, `catchain_network`, `block`)
- Added note about HTTP request logging not being available

## [0.2.2] - 2026-02-12

appVersion: `v0.1.2-mainnet`

### Fixed

- Metrics port is no longer exposed on public LoadBalancer services. A dedicated `<release>-metrics` ClusterIP service is created instead for internal scraping via ServiceMonitor or annotations.

## [0.2.0] - 2026-02-11

appVersion: `v0.1.2-mainnet`

### Added

- Grafana dashboard as TypeScript (Foundation SDK) in `grafana/`

### Changed

- Image tag pinned to specific version (no more floating `mainnet` tag)
- Init container image parameterized via `initImage.*`
- `pullPolicy` default changed from `Always` to `IfNotPresent`

## [0.1.0] - 2026-02-04

appVersion: `v0.1.0-mainnet`

Initial release.
