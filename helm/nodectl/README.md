# nodectl

> **Alpha software.** nodectl is under active development. Configuration format, CLI interface, and Helm chart values may change between releases without notice.

Helm chart for deploying nodectl on Kubernetes.

## Table of contents

- [What is nodectl](#what-is-nodectl)
- [Key concepts](#key-concepts)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Environment variables](#environment-variables)
- [Parameters](#parameters)
- [Architecture](#architecture)
- [Documentation](#documentation)
- [Useful commands](#useful-commands)

## What is nodectl

**nodectl** is a management daemon for TON validator nodes. It connects to one or more validators via the ADNL Control Server protocol and handles election participation, contract deployment, stake management, and network voting.

nodectl works in two modes:

- **Daemon** (`nodectl service`) — runs as the main container process, manages elections and contracts automatically
- **CLI** (`nodectl config ...`, `nodectl key ...`, `nodectl api ...`) — accessible via `kubectl exec` for configuration and diagnostics

## Key concepts

### Configuration model

nodectl uses four independent lists connected by bindings:

| Entity | Purpose |
|--------|---------|
| **Nodes** | ADNL connections to validator Control Servers |
| **Wallets** | Validator wallets for signing transactions |
| **Pools** | Single Nominator Pool contracts |
| **Bindings** | Map each node to a wallet and optionally a pool |

A binding ties everything together: node `node0` uses wallet `wallet0` and pool `pool0`. This decoupled model allows flexible mapping — wallets and pools are reusable, named independently from nodes.

### Single Nominator Pool (SNP)

nodectl currently supports **only** Single Nominator Pool contracts for staking. The SNP contract address is deterministic:

```
address = hash(snp_code + owner_address + validator_wallet_address)
```

This means **each node must have its own wallet**. If two nodes share a wallet (= same validator address), their pools get the same address, which breaks election participation. Always create one wallet per node.

When you add a pool with just the owner address (no explicit contract address), nodectl computes the SNP address automatically on startup from the owner and the bound validator wallet.

### Auto-deploy

The `contracts_task` runs in the background and automatically deploys contracts using the **master wallet**:

1. Deploys uninitialized validator wallets (1 TON each)
2. Deploys uninitialized nomination pools (1 TON each)
3. Tops up active wallets that fall below 5 TON (adds 10 TON)

The master wallet key is auto-generated in vault on first start. You only need to fund the master wallet address — contract deployment is automatic.

Total master wallet funding needed: `N * 1 TON` (wallets) + `N * 1 TON` (pools) + reserve. For 9 nodes, ~20 TON is sufficient for initial deployment.

### Vault

Vault stores private keys (wallet keys, control client keys). Currently only the file-based backend is documented:

| Backend | URL format | Use case |
|---------|-----------|----------|
| File-based | `file:///nodectl/data/vault.json&master_key=<hex>` | All setups |

Vault is configured via the **`VAULT_URL` environment variable**, not in `config.json`. The Helm chart passes this from a K8s Secret or plain value.

> **Tip:** Add `IPC_LOCK` capability to the container security context if you want file-based vault to use `mlock()` for memory protection. Not required — the service works without it.

## Installation

```bash
helm install my-nodectl oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl -f values.yaml
```

## Quick start

For a complete step-by-step guide covering deployment, configuration, key management, and funding, see [docs/setup.md](docs/setup.md).

## Environment variables

The chart sets these environment variables on the nodectl container:

| Variable | Source | Description |
|----------|--------|-------------|
| `VAULT_URL` | `vault.secretName` or `vault.url` | Vault connection string (required) |
| `CONFIG_PATH` | `dataPath` + `/config.json` | Path to config file on the PVC |

> **Do not edit the Parameters section by hand.** It is auto-generated from `@param` annotations in [values.yaml](values.yaml). To make changes, edit `values.yaml` and regenerate — see [docs/maintaining.md](docs/maintaining.md#updating-the-parameters-table-in-readme).

## Parameters

### General parameters

| Name | Description | Value |
|------|-------------|-------|
| `replicas` | Number of nodectl instances. Each replica shares the same PVC — do not scale beyond 1 unless using ReadWriteMany storage. | `1` |

### Image parameters

| Name | Description | Value |
|------|-------------|-------|
| `image.repository` | Container image repository | `ghcr.io/rsquad/ton-rust-node/nodectl` |
| `image.tag` | Image tag | `v0.1.0` |
| `image.pullPolicy` | Pull policy | `IfNotPresent` |
| `imagePullSecrets` | Registry pull secrets for private container images | `[]` |

### Container parameters

| Name | Description | Value |
|------|-------------|-------|
| `logLevel` | Logging level: trace, debug, info, warn, error | `info` |
| `logFile` | Path to log file inside the container. Null means stdout only. | `nil` |

### Port parameters

| Name | Description | Value |
|------|-------------|-------|
| `port` | HTTP API port. Used for health probes, REST API, and Swagger UI. | `8080` |

### Service parameters

| Name | Description | Value |
|------|-------------|-------|
| `service.type` | Service type for the HTTP API | `ClusterIP` |
| `service.annotations` | Annotations for the Service | `{}` |

### Storage parameters

| Name | Description | Value |
|------|-------------|-------|
| `storage.size` | PVC size for nodectl data | `1Gi` |
| `storage.storageClassName` | Storage class name. Empty string uses cluster default. | `""` |
| `storage.accessMode` | PVC access mode | `ReadWriteOnce` |

### Data directory

| Name | Description | Value |
|------|-------------|-------|
| `dataPath` | Directory inside the container where the PVC is mounted. Config, vault, and runtime state live here. | `/nodectl/data` |

### Vault parameters

| Name | Description | Value |
|------|-------------|-------|
| `vault.url` | Vault URL (plain text). Use `vault.secretName` instead for production. | `""` |
| `vault.secretName` | Name of an existing K8s Secret containing the vault URL. Takes precedence over `vault.url`. | `""` |
| `vault.secretKey` | Key inside the Secret that holds the vault URL. | `"VAULT_URL"` |

### Initial config parameters

| Name | Description | Value |
|------|-------------|-------|
| `initConfig.enabled` | Enable the init container that seeds config on first deploy | `true` |
| `initConfig.secretName` | Name of a Secret containing initial config (key: `config.json`). Leave empty to auto-generate. | `""` |

### Security parameters

| Name | Description | Value |
|------|-------------|-------|
| `securityContext` | Security context for the nodectl container. Optionally add IPC_LOCK capability for file-based vault mlock() support. | `{}` |

### Resource parameters

| Name | Description | Value |
|------|-------------|-------|
| `resources.requests.cpu` | CPU request | `100m` |
| `resources.requests.memory` | Memory request | `128Mi` |
| `resources.limits.cpu` | CPU limit | `500m` |
| `resources.limits.memory` | Memory limit | `256Mi` |

### Probe parameters

| Name | Description | Value |
|------|-------------|-------|
| `probes` | Liveness, readiness, and startup probes. Set to {} to disable all probes. | `{}` |

### Extra init containers

| Name | Description | Value |
|------|-------------|-------|
| `extraInitContainers` | Additional init containers to run before nodectl starts. | `[]` |

### Extra containers

| Name | Description | Value |
|------|-------------|-------|
| `extraContainers` | Additional sidecar containers to run alongside nodectl. | `[]` |

### Extra volumes

| Name | Description | Value |
|------|-------------|-------|
| `extraVolumes` | Additional volumes for the pod. | `[]` |
| `extraVolumeMounts` | Additional volume mounts for the nodectl container. | `[]` |

### Extra environment variables

| Name | Description | Value |
|------|-------------|-------|
| `extraEnv` | Additional environment variables for the nodectl container. | `[]` |
| `extraEnvFrom` | Additional envFrom sources for the nodectl container. | `[]` |

### Pod metadata parameters

| Name | Description | Value |
|------|-------------|-------|
| `podAnnotations` | Additional annotations for pods. Useful for Vault agent injection, service mesh, etc. | `{}` |
| `podLabels` | Additional labels for pods. | `{}` |

### Scheduling parameters

| Name | Description | Value |
|------|-------------|-------|
| `nodeSelector` | Node selector for pod scheduling | `{}` |
| `tolerations` | Tolerations for pod scheduling | `[]` |
| `affinity` | Affinity rules for pod scheduling | `{}` |

### ServiceAccount parameters

| Name | Description | Value |
|------|-------------|-------|
| `serviceAccount.enabled` | Create a ServiceAccount for the pods | `false` |
| `serviceAccount.name` | ServiceAccount name. Defaults to the release fullname if not set. | `""` |
| `serviceAccount.annotations` | Annotations for the ServiceAccount (e.g. for Vault or cloud IAM role binding) | `{}` |

### NetworkPolicy parameters

| Name | Description | Value |
|------|-------------|-------|
| `networkPolicy.enabled` | Create a NetworkPolicy. Restricts ingress to the HTTP API port. | `false` |
| `networkPolicy.allowCIDRs` | Source CIDRs allowed to reach the HTTP API. If empty, not restricted by source. | `[]` |
| `networkPolicy.extraIngress` | Additional raw ingress rules appended to the policy. | `[]` |

### PodDisruptionBudget parameters

| Name | Description | Value |
|------|-------------|-------|
| `podDisruptionBudget.enabled` | Create a PodDisruptionBudget | `false` |
| `podDisruptionBudget.minAvailable` | Minimum available pods during disruption. Only one of minAvailable or maxUnavailable should be set. | `1` |

### Debug parameters

| Name | Description | Value |
|------|-------------|-------|
| `debug.sleep` | Replace nodectl with sleep infinity for debugging | `false` |
| `debug.securityContext` | Security context overrides for debugging (e.g. SYS_PTRACE) | `{}` |

## Architecture

The chart creates:

- **Deployment** (strategy: Recreate) running nodectl as a daemon
- **PersistentVolumeClaim** for nodectl data — config, file vault, and runtime state
- **Service** (ClusterIP) exposing the HTTP API port

Optional resources (created when enabled):

- **Init container** — generates default config on first deploy (or seeds from a K8s Secret)
- **ServiceAccount** — for Vault agent injection or cloud IAM role binding
- **NetworkPolicy** — restricts ingress to the HTTP API port
- **PodDisruptionBudget** — prevents all pods from being evicted simultaneously

### Data persistence

All nodectl data lives on a PVC mounted at `dataPath` (default: `/nodectl/data`):

- `config.json` — nodectl configuration
- `vault.json` — encrypted secrets (file-based vault)
- Runtime state written by nodectl

The PVC is writable and persists across pod restarts and upgrades.

The service automatically detects and reloads config changes — no restart needed after modifying `config.json`.

### Init container

On first deploy, the init container prepares the PVC:

1. If `config.json` exists on the PVC — skip (existing config preserved)
2. If `initConfig.secretName` is set — copy config from the K8s Secret
3. Otherwise — run `nodectl config generate` to create a default config

### Container command

| Condition | Command |
|-----------|---------|
| `debug.sleep: true` | `sleep infinity` |
| default | `nodectl --verbose=<logLevel> service --config=<dataPath>/config.json` |

## Documentation

| Topic | Document |
|-------|----------|
| Step-by-step validator setup | [docs/setup.md](docs/setup.md) |
| Elections and stake policies | [docs/elections.md](docs/elections.md) |
| Chart maintainer guide | [docs/maintaining.md](docs/maintaining.md) |

## Useful commands

```bash
# Check pod status
kubectl get pods -l app.kubernetes.io/name=nodectl

# View logs
kubectl logs deploy/my-nodectl -f

# Exec into pod for CLI access
kubectl exec -it deploy/my-nodectl -- sh

# Check binding status
kubectl exec deploy/my-nodectl -- nodectl config elections show

# Check service health
kubectl exec deploy/my-nodectl -- nodectl api health

# Port-forward to access Swagger UI
kubectl port-forward deploy/my-nodectl 8080:8080
# Open http://localhost:8080/swagger
```
