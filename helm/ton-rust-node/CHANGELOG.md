# Changelog

All notable changes to the Helm chart will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the Helm chart release tags (e.g. `helm/v0.3.0`).

## [0.4.0] - 2026-02-27

appVersion: `v0.2.1-mainnet`

### Added

- `vault.url` / `vault.secretName` / `vault.secretKey` — secrets vault configuration via `VAULT_URL` env var. The `secrets_vault_config` field in `config.json` is no longer supported; use the chart's vault values instead. See [docs/vault.md](docs/vault.md)
- `services.<port>.labels` — custom labels for all service types (adnl, control, liteserver, jsonRpc). ADNL also supports per-replica label overrides via `perReplica[].labels`

### Fixed

- `extraContainers` and `extraInitContainers` now support Helm templating (`.Release.Name`, `.Values.*`, named templates)
- `nodeConfigs` values now support Helm templating

### Changed

- **Breaking:** NetworkPolicy redesigned with per-port ingress rules. `networkPolicy.allowCIDRs` removed — use per-port `allowFrom` instead. TCP ports (control, liteserver, jsonRpc, metrics) now require explicit `.enabled: true`. ADNL remains always open (public by default)
- **Breaking:** Chart renamed from `ton-rust-node` to `node`. This is a monorepo consolidation — all artifacts now live under the `ton-rust-node/*` namespace
- **Breaking:** Chart OCI registry moved from `oci://ghcr.io/rsquad/helm/ton-rust-node` to `oci://ghcr.io/rsquad/ton-rust-node/helm/node`
- **Breaking:** Default image repository changed from `ghcr.io/rsquad/ton-rust-node` to `ghcr.io/rsquad/ton-rust-node/node`
- `app.kubernetes.io/name` label changed from `ton-rust-node` to `node`
- Default image tag updated to `v0.2.1-mainnet`

## [0.3.2] - 2026-02-24

appVersion: `v0.1.2-mainnet`

### Added

- `storage.<volume>.resourcePolicy` — configurable `helm.sh/resource-policy` annotation on volumeClaimTemplates. Defaults: `keep` for main/keys, omitted for db/logs
- `storage.<volume>.annotations` — extra annotations per volume PVC

## [0.3.1] - 2026-02-18

appVersion: `v0.1.2-mainnet`

### Added

- `imagePullSecrets` — support for private container registries

### Fixed

- Documentation link in NOTES.txt pointed to the old `ton-devops` repository

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

- `serviceAccount.create` renamed to `serviceAccount.enabled` for consistency with other feature toggles
- `services` restructured to per-port configuration — each port (adnl, control, liteserver, jsonRpc) gets its own Service with independent type, annotations, and perReplica overrides
- `hostPort` is now per-port: `hostPort.adnl`, `hostPort.control`, `hostPort.liteserver`, `hostPort.jsonRpc`, `hostPort.metrics`
- Control port defaults to ClusterIP; liteserver and jsonRpc default to LoadBalancer

### Fixed

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
