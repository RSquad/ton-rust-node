# ton-rust-node

Helm chart for deploying TON Rust Node on Kubernetes.

> **Current status:** Only a **mainnet fullnode** image is published (`ghcr.io/rsquad/ton-rust-node:mainnet`). Validator support is planned but not yet available.

## Table of contents

- [Node roles](#node-roles)
- [Installation](#installation)
- [Quick start](#quick-start)
- [Providing configuration](#providing-configuration)
- [Parameters](#parameters)
- [Architecture](#architecture)
- [Configuration guide](#configuration-guide)
- [Useful commands](#useful-commands)

## Node roles

A TON node can run in two roles — **validator** or **fullnode** — using the same binary and the same chart. The difference is in configuration and how you use the node.

**Validator** participates in network consensus: it validates blocks, votes in elections, and earns rewards. A validator is a critical infrastructure component, so:

- Never expose `liteserver` or `jsonRpc` ports on a validator. Every open port is an attack surface and adds unnecessary load to a machine that must stay performant and stable.
- Allocate more resources (see [docs/resources.md](docs/resources.md) for recommended values).

**Fullnode** syncs the blockchain and can serve queries to external clients. This is the role you want for building APIs, explorers, bots, and any other integration. Enable `liteserver` and/or `jsonRpc` ports to expose the node's data. A fullnode can be:

- **Regular** — keeps only recent state. Suitable for most API use cases.
- **Archival** — stores the full history of the blockchain. Requires significantly more disk space. See [docs/node-config.md](docs/node-config.md) for the relevant config fields.

We recommend running validators and fullnodes as **separate Helm releases** so they have independent configs, resources, and lifecycle:

```bash
helm install validator ./helm/ton-rust-node -f validator-values.yaml
helm install fullnode ./helm/ton-rust-node -f fullnode-values.yaml
```

## Installation

```bash
# From local chart
helm install my-node ./helm/ton-rust-node -f values.yaml

# From OCI registry
helm install my-node oci://ghcr.io/rsquad/helm/ton-rust-node --version 0.1.0 -f values.yaml
```

## Quick start

Minimal deployment:

```yaml
# values.yaml
replicas: 2

services:
  perReplica:
    - annotations:
        metallb.universe.tf/loadBalancerIPs: "1.2.3.4"
    - annotations:
        metallb.universe.tf/loadBalancerIPs: "5.6.7.8"

nodeConfigs:
  node-0.json: |
    { "log_config_name": "/main/logs.config.yml", ... }
  node-1.json: |
    { "log_config_name": "/main/logs.config.yml", ... }
```

The chart ships with a mainnet `globalConfig` and a sensible `logsConfig` by default — you only need `nodeConfigs` and service annotations. The examples above use MetalLB — see [docs/networking.md](docs/networking.md) for other options (NodePort, hostNetwork, ingress-nginx).

```bash
helm install my-validator ./helm/ton-rust-node -f values.yaml
```

With liteserver and JSON-RPC ports (2 replicas):

```yaml
replicas: 2

ports:
  liteserver: 40000
  jsonRpc: 8081

services:
  perReplica:
    - annotations:
        metallb.universe.tf/loadBalancerIPs: "10.0.0.1"
    - annotations:
        metallb.universe.tf/loadBalancerIPs: "10.0.0.2"

nodeConfigs:
  node-0.json: |
    { "log_config_name": "/main/logs.config.yml", ... }
  node-1.json: |
    { "log_config_name": "/main/logs.config.yml", ... }
```

Multiple nodes in the same namespace — use different release names:

```bash
helm install validator ./helm/ton-rust-node -f validator-values.yaml
helm install lite ./helm/ton-rust-node -f lite-values.yaml
```

This creates separate StatefulSets (`validator`, `lite`), services (`validator-0`, `lite-0`), and configs.

## Providing configuration

Every config (global config, logs config, node configs, basestate, zerostate) supports three modes:

### 1. Inline in values

Pass the content directly:

```yaml
globalConfig: |
  {"dht": {"nodes": [...]}}

logsConfig: |
  refresh_rate: 30 seconds
  ...

nodeConfigs:
  node-0.json: |
    {"log_config_name": "/main/logs.config.yml", ...}
  node-1.json: |
    {"log_config_name": "/main/logs.config.yml", ...}
```

### 2. From local files via `--set-file`

Keep configs as separate files on disk and let Helm read them:

```bash
helm install my-node ./helm/ton-rust-node \
  --set-file globalConfig=./global.config.json \
  --set-file logsConfig=./logs.config.yml \
  --set-file nodeConfigs.node-0\\.json=./configs/node-0.json \
  --set-file nodeConfigs.node-1\\.json=./configs/node-1.json \
  -f values.yaml
```

Note the escaped dot (`\\.`) in `node-0.json` keys — required by Helm's `--set` parser.

### 3. Reference existing Kubernetes resources

Point to ConfigMaps/Secrets that already exist in the cluster:

```yaml
existingGlobalConfigMapName: my-global-config
existingLogsConfigMapName: my-logs-config
existingNodeConfigsSecretName: my-node-secrets
existingBasestateConfigMapName: my-basestate
existingZerostateConfigMapName: my-zerostate
```

When an `existing*Name` is set, the chart does not create that resource — it only references it in the StatefulSet volumes. The inline value (e.g. `globalConfig`) is ignored.

> **Why no file path option?** Helm's `.Files.Get` can only read files bundled inside the chart package — it cannot access files from your filesystem at install time. That's why we offer three modes instead of a simple file path. If you prefer to keep configs as local files, use `--set-file` (mode 2) or clone the chart and place files inside the chart directory.

## Parameters

> **Do not edit this section by hand.** It is auto-generated from `@param` annotations in [values.yaml](values.yaml). To make changes, edit `values.yaml` and regenerate — see [docs/maintaining.md](docs/maintaining.md#updating-the-parameters-table-in-readme).

### General parameters

| Name       | Description                                                                                                                                                                                                                                                   | Value |
| ---------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| `replicas` | Number of node instances in the StatefulSet. Each replica is an independent TON node with its own config, keys, and IP — not replication for redundancy. You need a matching nodeConfigs entry (node-N.json) and a perReplica service entry for each replica. | `1`   |
| `command`  | Override container command. Auto-detected: adds `-z /main/static` when zerostate+basestate are provided. Change only if you know what you are doing.                                                                                                          | `[]`  |

### Image parameters

| Name               | Description                | Value                          |
| ------------------ | -------------------------- | ------------------------------ |
| `image.repository` | Container image repository | `ghcr.io/rsquad/ton-rust-node` |
| `image.tag`        | Image tag                  | `mainnet`                      |
| `image.pullPolicy` | Pull policy                | `Always`                       |

### Resource parameters

| Name                        | Description    | Value  |
| --------------------------- | -------------- | ------ |
| `resources.requests.cpu`    | CPU request    | `8`    |
| `resources.requests.memory` | Memory request | `32Gi` |
| `resources.limits.cpu`      | CPU limit      | `16`   |
| `resources.limits.memory`   | Memory limit   | `64Gi` |

### Storage parameters

| Name                            | Description                                                    | Value        |
| ------------------------------- | -------------------------------------------------------------- | ------------ |
| `storage.main.size`             | Main volume size                                               | `1Gi`        |
| `storage.main.storageClassName` | Storage class for main volume                                  | `local-path` |
| `storage.db.size`               | Database volume size (hundreds of GB for mainnet)              | `1Ti`        |
| `storage.db.storageClassName`   | Storage class for database volume                              | `local-path` |
| `storage.logs.enabled`          | Create a PVC for logs. Set to false if you log to stdout only. | `true`       |
| `storage.logs.size`             | Logs volume size                                               | `150Gi`      |
| `storage.logs.storageClassName` | Storage class for logs volume                                  | `local-path` |
| `storage.keys.size`             | Keys volume size                                               | `1Gi`        |
| `storage.keys.storageClassName` | Storage class for keys volume                                  | `local-path` |

### Port parameters

| Name               | Description                                 | Value   |
| ------------------ | ------------------------------------------- | ------- |
| `ports.adnl`       | ADNL port (UDP)                             | `30303` |
| `ports.control`    | Control port (TCP). Set to null to disable. | `50000` |
| `ports.liteserver` | Liteserver port (TCP). Set to enable.       | `nil`   |
| `ports.jsonRpc`    | JSON-RPC port (TCP). Set to enable.         | `nil`   |

### Service parameters

| Name                             | Description                                                                                                                            | Value          |
| -------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- | -------------- |
| `services.type`                  | Service type                                                                                                                           | `LoadBalancer` |
| `services.externalTrafficPolicy` | Traffic policy                                                                                                                         | `Local`        |
| `services.annotations`           | Annotations applied to ALL per-replica services                                                                                        | `{}`           |
| `services.perReplica`            | Per-replica service overrides. List index = replica index. Annotations are merged with the shared ones (per-replica wins on conflict). | `[]`           |

### Configuration parameters

| Name                             | Description                                                                                                                   | Value             |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- | ----------------- |
| `nodeConfigs`                    | Per-node JSON configs (one node-N.json per replica). See docs/node-config.md.                                                 | `{}`              |
| `existingNodeConfigsSecretName`  | Use an existing Secret for node configs instead of inline                                                                     | `""`              |
| `globalConfig`                   | Global TON network config (JSON string). A mainnet default is bundled in files/global.config.json. See docs/global-config.md. | `bundled mainnet` |
| `existingGlobalConfigMapName`    | Use an existing ConfigMap for global config instead of inline                                                                 | `""`              |
| `logsConfig`                     | Logging configuration (log4rs YAML). A default is bundled in files/logs.config.yml. See docs/logging.md.                      | `bundled default` |
| `existingLogsConfigMapName`      | Use an existing ConfigMap for logs config instead of inline                                                                   | `""`              |
| `basestate`                      | Base64-encoded basestate.boc. Only needed when bootstrapping a brand new network.                                             | `""`              |
| `existingBasestateConfigMapName` | Use an existing ConfigMap for basestate                                                                                       | `""`              |
| `zerostate`                      | Base64-encoded zerostate.boc. Only needed when bootstrapping a brand new network.                                             | `""`              |
| `existingZerostateConfigMapName` | Use an existing ConfigMap for zerostate                                                                                       | `""`              |

### Probe parameters

| Name     | Description                                                                                                                                                                         | Value |
| -------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----- |
| `probes` | Liveness, readiness, and startup probes. The TON node has no built-in health endpoint; if jsonRpc is enabled you can use /getMasterchainInfo as a basic check. Disabled by default. | `{}`  |

### Networking parameters

| Name          | Description                                                                                                                                                                                            | Value   |
| ------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------- |
| `hostNetwork` | Bind pods directly to the host network. The pod gets the node's IP with zero NAT overhead. Requires one pod per node — use nodeSelector or podAntiAffinity to spread replicas. See docs/networking.md. | `false` |

### Scheduling parameters

| Name           | Description                       | Value |
| -------------- | --------------------------------- | ----- |
| `nodeSelector` | Node selector for pod scheduling  | `{}`  |
| `tolerations`  | Tolerations for pod scheduling    | `[]`  |
| `affinity`     | Affinity rules for pod scheduling | `{}`  |

### PodDisruptionBudget parameters

| Name                               | Description                                                                                         | Value   |
| ---------------------------------- | --------------------------------------------------------------------------------------------------- | ------- |
| `podDisruptionBudget.enabled`      | Create a PodDisruptionBudget                                                                        | `false` |
| `podDisruptionBudget.minAvailable` | Minimum available pods during disruption. Only one of minAvailable or maxUnavailable should be set. | `1`     |

### Debug parameters

| Name                    | Description                                                | Value   |
| ----------------------- | ---------------------------------------------------------- | ------- |
| `debug.sleep`           | Replace node with sleep infinity for debugging             | `false` |
| `debug.securityContext` | Security context overrides for debugging (e.g. SYS_PTRACE) | `{}`    |

## Architecture

The chart creates:

- **StatefulSet** named after the release, with `podManagementPolicy: Parallel` and `fsGroup: 1000`
- **One LoadBalancer Service per replica** with `externalTrafficPolicy: Local` and optional static IPs
- **Init container** (`alpine:3.23`, pinned by digest) that seeds configs from volumes into the main PVC
- **PersistentVolumeClaims**: `main`, `db`, `keys`, and optionally `logs` (see `storage.logs.enabled`)
- **ConfigMaps** for global config, logs config, and optionally basestate/zerostate
- **Secret** for per-node JSON configs

All resource names are prefixed with the release name, allowing multiple installations in the same namespace.

### Volumes and mounts

The node uses persistent volumes:

| Volume | Mount path | Purpose | Optional |
|--------|------------|---------|----------|
| `main` | `/main` | Working directory: node config, global config, logs config, static files (basestate/zerostate hashes) | no |
| `db` | `/db` | Blockchain database (the largest volume, grows over time) | no |
| `logs` | `/logs` | Rolling log files (output.log, rotated by log4rs) | yes (`storage.logs.enabled`) |
| `keys` | `/keys` | Node keys and vault | no |

#### Storage recommendations

> **Important:** Disk performance is critical for correct node operation. The `db` volume requires storage capable of sustaining **up to 64,000 IOPS**. Insufficient disk performance leads to sync delays, missed validations, and degraded node behavior. Use NVMe or high-performance SSD with a local volume provisioner.

The `db` and `logs` volumes are performance-critical — they handle continuous heavy I/O from the blockchain database and log writes. **We strongly recommend using local storage** that provides direct disk access: `local-path`, OpenEBS LVM, or similar local volume provisioners. Network-attached storage (NFS, Ceph RBD, EBS, etc.) adds latency that significantly impacts node performance and sync speed.

For `main` and `keys` volumes the I/O load is minimal — any storage provider will work. We recommend [Longhorn](https://longhorn.io/) v1 with replica count 3 for data safety. We have tested Longhorn v2 and do not recommend it at this time.

| Volume | Default size | Notes |
|--------|-------------|-------|
| `db` | `1Ti` | Not recommended to go below `500Gi`. Grows over time as the blockchain state accumulates. |
| `logs` | `150Gi` | Default log rotation is configured for 25 GB per file with 4 rotations (see `logsConfig`). You can reduce the volume size if you adjust the rolling file limits accordingly. See [docs/logging.md](docs/logging.md) for details. |
| `main` | `1Gi` | Holds configs and static files. Default is sufficient. |
| `keys` | `1Gi` | Holds node keys and vault. Default is sufficient. |

### Init container

Before the node starts, an init container (`alpine:3.23`, pinned by digest) runs a bootstrap script that prepares the `/main` volume:

1. Copies `global.config.json` and `logs.config.yml` from seed ConfigMaps into `/main`
2. If basestate/zerostate are provided — hashes them with SHA-256 and places as `/main/static/{hash}.boc`
3. Resolves the pod index from the pod name (e.g. `my-node-2` -> `2`) and copies `node-2.json` from the node-configs Secret as `/main/config.json`
4. Sets ownership to UID `1000` (non-root app user)

Seed volumes (ConfigMaps/Secrets) are mounted read-only under `/seed/`:

| Seed volume | Mount path | Source |
|-------------|------------|--------|
| `global-config` | `/seed/global-config` | ConfigMap with `global.config.json` |
| `logs-config` | `/seed/logs-config` | ConfigMap with `logs.config.yml` |
| `node-configs` | `/seed/node-configs` | Secret with `node-{i}.json` per replica |
| `basestate` | `/seed/basestate` | ConfigMap with `basestate.boc` (optional) |
| `zerostate` | `/seed/zerostate` | ConfigMap with `zerostate.boc` (optional) |

### Container command

The main container command is determined automatically:

| Condition | Command |
|-----------|---------|
| `debug.sleep: true` | `sleep infinity` (busybox image) |
| `command` is set | custom command |
| basestate + zerostate provided | `node -c /main -z /main/static` |
| default | `node -c /main` |

### Config change detection

A SHA-256 checksum of all inline configs is stored in the pod annotation `rsquad.io/config-checksum`. Any config change triggers a pod restart.

## Configuration guide

This chart does **not** generate node configs — you must prepare them yourself. The node uses three config files:

| Config | Description | Default | Reference |
|--------|-------------|---------|-----------|
| `globalConfig` | TON network config (DHT nodes, network ID, etc.) | mainnet (bundled) | [docs/global-config.md](docs/global-config.md) |
| `logsConfig` | log4rs logging config (appenders, levels, rotation) | bundled | [docs/logging.md](docs/logging.md) |
| `nodeConfigs` | Per-node config (IP, ports, keys, paths) — one `node-N.json` per replica | **none, required** | [docs/node-config.md](docs/node-config.md) |

Sensible defaults for `globalConfig` (mainnet, from [ton-blockchain.github.io](https://ton-blockchain.github.io/)) and `logsConfig` are bundled in the chart and used automatically. It is strongly recommended to provide your own up-to-date `globalConfig`. `nodeConfigs` has no default — the chart will fail with a clear error if it is not provided.

See also [docs/resources.md](docs/resources.md) for CPU and memory recommendations and [docs/networking.md](docs/networking.md) for networking modes (LoadBalancer, NodePort, hostNetwork, ingress-nginx).

See the linked docs for field-by-field explanations, required fields, and which values must match between the config files and the Helm values (e.g. ports, IPs).

For chart maintainers: [docs/maintaining.md](docs/maintaining.md) documents how to regenerate the Parameters table after editing `values.yaml`.

## Useful commands

```bash
# Check pod status (replace "my-node" with your release name)
kubectl get pods -l app.kubernetes.io/name=ton-rust-node,app.kubernetes.io/instance=my-node

# Get external service IPs
kubectl get svc -l app.kubernetes.io/name=ton-rust-node,app.kubernetes.io/instance=my-node

# View logs
kubectl logs my-node-0 -c ton-node

# Exec into pod
kubectl exec -it my-node-0 -c ton-node -- /bin/sh
```
