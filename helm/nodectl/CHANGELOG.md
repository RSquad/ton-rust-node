# Changelog

All notable changes to the nodectl Helm chart will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the Helm chart release tags (e.g. `helm/nodectl/v0.1.0`).

## [0.3.1] - 2026-05-25

appVersion: `v0.5.1`

### Changed

- Default image updated to nodectl `v0.5.1`

## [0.3.0] - 2026-05-19

appVersion: `v0.5.0`

### Added

- Attach an existing `ServiceAccount` to the Pod by setting `serviceAccount.name` while keeping `serviceAccount.enabled=false`. Useful when the SA is managed outside the Helm release (e.g. bound to a HashiCorp Vault role).
- Documentation: HashiCorp Vault backend in `docs/setup.md` and the file → HashiCorp migration runbook in `docs/copy-file-to-hashicorp.md`.

### Changed

- Default image updated to nodectl `v0.5.0`

## [0.2.1] - 2026-04-21

appVersion: `v0.4.0`

### Changed

- Default image updated to nodectl `v0.4.0`

## [0.2.0] - 2026-03-24

appVersion: `v0.3.0`

### Added

- `service.nodePort` — fixed node port when `service.type` is `NodePort`
- `service.clusterIP` — explicit ClusterIP (set to `None` for headless)
- `service.loadBalancerIP` — static IP for cloud load balancers
- `service.externalTrafficPolicy` — `Local` or `Cluster` for NodePort/LoadBalancer
- `networkPolicy.allowFrom` — flexible network policy peers (ipBlock, podSelector, namespaceSelector)

### Changed

- Default image updated to nodectl `v0.3.0`

### Removed

- `networkPolicy.allowCIDRs` — replaced by `networkPolicy.allowFrom` which accepts standard NetworkPolicy peers

## [0.1.4] - 2026-03-19

appVersion: `v0.2.1`

### Changed

- Default image updated to nodectl `v0.2.1`

### Fixed

- Vault URL examples used `&` separator instead of `?`

## [0.1.3] - 2026-03-05

appVersion: `v0.2.0`

### Changed
- Default image updated to nodectl `v0.2.0`
- Removed `logLevel` and `logFile` chart values — nodectl now manages logging through its own `config log` commands
- nodectl container now starts with `nodectl service --config=<dataPath>/config.json` (without `--verbose`/`--log-file` args)

## [0.1.2] - 2026-02-27

appVersion: `v0.1.1`

### Changed
- Default image tag updated to `v0.1.1` (V1R3 wallet support)

## [0.1.1] - 2026-02-24

appVersion: `v0.1.0`

### Added

- `storage.resourcePolicy` — configurable `helm.sh/resource-policy` annotation on the PVC, defaults to `keep` to prevent accidental deletion on `helm uninstall`
- `storage.annotations` — extra annotations for the PVC

## [0.1.0] - 2026-02-22

appVersion: `v0.1.0`

Initial release.
