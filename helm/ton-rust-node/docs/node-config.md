# Node config (config.json)

Each TON node replica requires its own `config.json` — the main configuration file that defines ADNL networking, database paths, garbage collection, collator behavior, and optional control/liteserver/JSON-RPC endpoints.

In this chart, per-node configs are provided via the `nodeConfigs` map in values (or an existing Secret). The keys must follow the naming convention `node-N.json` where N matches the StatefulSet replica index (0-based). At startup, the init container copies `node-<pod-index>.json` into `/main/config.json`.

> **Note on keys:** All keys in the config are 256-bit Ed25519 keys encoded as base64 strings. Private keys (ADNL, control server, liteserver) are stored as plaintext in `config.json`. This is acceptable for fullnode deployments. For validators, [nodectl](../../nodectl/README.md) provides an encrypted vault for key management.

## Table of contents

- [Helm integration constraints](#helm-integration-constraints)
- [Providing configs](#providing-configs)
- [Minimal example (fullnode / liteserver)](#minimal-example-fullnode--liteserver)
- [Minimal example (validator)](#minimal-example-validator)
- [Generating keys](#generating-keys)
- [Archival node](#archival-node)
- [Field reference](#field-reference)
- [Validator-specific sections](#validator-specific-sections)
- [Advanced fields](#advanced-fields)

> **See also:** For the full catalog of Prometheus metrics and naming convention see [metrics.md](metrics.md).


## Helm integration constraints

Several fields in the node config must be consistent with Helm values:

| Field | Must match |
|-------|------------|
| `adnl_node.ip_address` | The external IP assigned to this replica's LoadBalancer service, port = `ports.adnl` |
| `control_server.address` | Port must match `ports.control` (if enabled) |
| `lite_server.address` | Port must match `ports.liteserver` (if enabled) |
| `json_rpc_server.address` | Port must match `ports.jsonRpc` (if enabled) |
| `metrics.address` | Port must match `ports.metrics` (if enabled) |
| `log_config_name` | Must be `/main/logs.config.yml` (where the chart mounts the logs config) |
| `ton_global_config_name` | Must be `/main/global.config.json` (where the chart mounts the global config) |
| `internal_db_path` | Must be `/db` (where the chart mounts the db PVC) |

## Providing configs

```bash
# Inline in values
helm install my-node ./helm/ton-rust-node \
  --set-file 'nodeConfigs.node-0\.json=./node-0.json' \
  ...

# Or in values.yaml
nodeConfigs:
  node-0.json: |
    { "log_config_name": "/main/logs.config.yml", ... }
```

Or reference a pre-existing Secret:

```yaml
existingNodeConfigsSecretName: my-node-configs
```

## Minimal example (fullnode / liteserver)

A typical fullnode exposes `lite_server` for lite-client queries and `json_rpc_server` for the HTTP API. `control_server` is optional.

> **Note:** The examples below show **recommended production values**, which may differ from code defaults listed in the [field reference](#field-reference). If a field is omitted from the config, the node uses the code default.

```json
{
  "log_config_name": "/main/logs.config.yml",
  "ton_global_config_name": "/main/global.config.json",
  "internal_db_path": "/db",
  "sync_by_archives": true,
  "states_cache_mode": "Moderate",
  "adnl_node": {
    "ip_address": "<your-external-ip>:30303",
    "keys": [
      { "tag": 1, "data": { "type_id": 1209251014, "pvt_key": "<dht-private-key-base64>" } },
      { "tag": 2, "data": { "type_id": 1209251014, "pvt_key": "<overlay-private-key-base64>" } }
    ]
  },
  "lite_server": {
    "address": "0.0.0.0:40000",
    "server_key": { "type_id": 1209251014, "pvt_key": "<liteserver-private-key-base64>" }
  },
  "json_rpc_server": {
    "address": "0.0.0.0:8081"
  },
  "metrics": {
    "address": "0.0.0.0:9100",
    "global_labels": { "network": "mainnet", "node_id": "lite-0" }
  },
  "gc": {
    "enable_for_archives": true,
    "archives_life_time_hours": 48,
    "enable_for_shard_state_persistent": true,
    "cells_gc_config": {
      "gc_interval_sec": 900,
      "cells_lifetime_sec": 86400
    }
  },
  "cells_db_config": {
    "states_db_queue_len": 1000,
    "prefill_cells_counters": false,
    "cells_cache_size_bytes": 4000000000,
    "counters_cache_size_bytes": 4000000000
  }
}
```

## Minimal example (validator)

A validator needs `control_server` for key management and election participation. Liteserver and JSON-RPC are not needed on a validator and should be kept separate for security. The `collator_config` tunes block production.

```json
{
  "log_config_name": "/main/logs.config.yml",
  "ton_global_config_name": "/main/global.config.json",
  "internal_db_path": "/db",
  "sync_by_archives": true,
  "states_cache_mode": "Moderate",
  "adnl_node": {
    "ip_address": "<your-external-ip>:30303",
    "keys": [
      { "tag": 1, "data": { "type_id": 1209251014, "pvt_key": "<dht-private-key-base64>" } },
      { "tag": 2, "data": { "type_id": 1209251014, "pvt_key": "<overlay-private-key-base64>" } }
    ]
  },
  "control_server": {
    "address": "0.0.0.0:50000",
    "server_key": { "type_id": 1209251014, "pvt_key": "<control-server-private-key-base64>" },
    "clients": {
      "list": [
        { "type_id": 1209251014, "pub_key": "<control-client-public-key-base64>" }
      ]
    }
  },
  "metrics": {
    "address": "0.0.0.0:9100",
    "global_labels": { "network": "mainnet", "node_id": "validator-0" }
  },
  "collator_config": {
    "cutoff_timeout_ms": 1000,
    "stop_timeout_ms": 1500,
    "max_collate_threads": 10,
    "retry_if_empty": false,
    "finalize_empty_after_ms": 800,
    "empty_collation_sleep_ms": 100,
    "external_messages_maximum_queue_length": 25600
  },
  "gc": {
    "enable_for_archives": true,
    "archives_life_time_hours": 48,
    "enable_for_shard_state_persistent": true,
    "cells_gc_config": {
      "gc_interval_sec": 900,
      "cells_lifetime_sec": 86400
    }
  },
  "cells_db_config": {
    "states_db_queue_len": 1000,
    "prefill_cells_counters": false,
    "cells_cache_size_bytes": 4000000000,
    "counters_cache_size_bytes": 4000000000
  }
}
```

## Generating keys

Each node needs several Ed25519 key pairs. The config references them as base64-encoded 32-byte private keys. You generate a separate key pair for every purpose: DHT, overlay, liteserver, control server, and control client.

All keys in the config use the same structure:

```json
{ "type_id": 1209251014, "pvt_key": "<base64-encoded-32-byte-private-key>" }
```

The `type_id` value `1209251014` means Ed25519 — this is the only supported key type. Public keys (e.g. in `control_server.clients`) use the same format but with `pub_key` instead of `pvt_key`.

### Step-by-step

1. **Generate a raw 32-byte Ed25519 private key** using OpenSSL:

   ```bash
   openssl genpkey -algorithm ed25519 -outform DER | tail -c 32 | base64
   ```

   This outputs a base64 string like `GnEN3s5t2Z3W1e...==` — this is your `pvt_key`.

2. **Derive the public key** from the same private key (needed for `control_server.clients` and for publishing the liteserver key in the global config):

   ```bash
   openssl genpkey -algorithm ed25519 -outform DER > /tmp/ed25519.der
   # private key (base64):
   tail -c 32 /tmp/ed25519.der | base64
   # public key (base64):
   openssl pkey -inform DER -in /tmp/ed25519.der -pubout -outform DER | tail -c 32 | base64
   ```

3. **Repeat** for each key you need. A typical fullnode with liteserver needs 3 key pairs:

   | Key | Used in | Field |
   |-----|---------|-------|
   | DHT private key | `adnl_node.keys[0]` (tag 1) | `pvt_key` |
   | Overlay private key | `adnl_node.keys[1]` (tag 2) | `pvt_key` |
   | Liteserver private key | `lite_server.server_key` | `pvt_key` |

   A validator additionally needs:

   | Key | Used in | Field |
   |-----|---------|-------|
   | Control server private key | `control_server.server_key` | `pvt_key` |
   | Control client key pair | `control_server.clients.list[0]` | `pub_key` (public part only) |

### Quick generation script

Generate all keys at once for a fullnode with liteserver:

```bash
#!/bin/bash
for name in dht overlay liteserver; do
  openssl genpkey -algorithm ed25519 -outform DER > /tmp/${name}.der
  pvt=$(tail -c 32 /tmp/${name}.der | base64)
  pub=$(openssl pkey -inform DER -in /tmp/${name}.der -pubout -outform DER | tail -c 32 | base64)
  echo "${name}:"
  echo "  pvt_key: ${pvt}"
  echo "  pub_key: ${pub}"
  rm /tmp/${name}.der
done
```

For a validator, add `control-server` and `control-client` to the loop.

> **Important:** Each key must be unique. Do not reuse the same key for different purposes (e.g. DHT and overlay). Do not share keys between different nodes.


## Archival node

By default the node prunes old archives and state snapshots via the `gc` section. To keep the **full** blockchain history, override the GC settings in your node config:

```json
{
  "gc": {
    "enable_for_archives": false,
    "enable_for_shard_state_persistent": false,
    "cells_gc_config": {
      "gc_interval_sec": 900,
      "cells_lifetime_sec": 86400
    }
  },
  "skip_saving_persistent_states": false
}
```

The key points:

- `enable_for_archives: false` — stop deleting block archives. Without this, archives older than `archives_life_time_hours` are pruned. When disabled, `archives_life_time_hours` is ignored entirely.
- `enable_for_shard_state_persistent: false` — stop GC from pruning persistent state snapshots. Note that even with GC enabled, the node uses a smart thinning strategy rather than deleting everything: older states are kept at progressively lower frequency, so there is always some state available at any point in time. Disabling GC preserves all snapshots, which is useful for serving queries against old states but consumes significantly more disk space.
- `skip_saving_persistent_states: false` — make sure snapshots are actually created. If set to `true`, the node never saves them regardless of GC settings.
- **Do not disable `cells_gc_config`.** Cells GC removes unreferenced cells (reference counting) — it does not delete blocks or states. Turning it off leads to DB storage leaks.

Full mainnet history is in the terabytes range and grows continuously. Make sure `storage.db.size` is large enough.

## Field reference

### Top-level fields

#### `log_config_name`

Path to the log4rs YAML config. Relative paths are resolved from the config directory.

| Type | Required | Default |
|------|----------|---------|
| string \| null | no | `null` (falls back to console output at `info` level) |

In this chart, always set to `"/main/logs.config.yml"`. See [logging.md](logging.md) for the log config format.

#### `ton_global_config_name`

Path to the global network config (DHT nodes, zero state, hardforks). Determines which network the node joins.

| Type | Required | Default |
|------|----------|---------|
| string \| null | yes | `null` |

In this chart, always set to `"/main/global.config.json"`. See [global-config.md](global-config.md).

#### `internal_db_path`

Path to the node's internal database directory. Stores blocks, states, indexes, and other data. Requires significant disk space (hundreds of GB for mainnet).

| Type | Required | Default |
|------|----------|---------|
| string \| null | no | `"node_db"` |

In this chart, always set to `"/db"` (where the db PVC is mounted).

#### `restore_db`

Enable database integrity check and repair on startup.

| Type | Required | Default |
|------|----------|---------|
| bool | no | `false` |

#### `boot_from_zerostate`

If `true`, the node syncs from the zero state (genesis) instead of using the `init_block` from the global config. Much slower but verifies the full chain from the beginning.

| Type | Required | Default |
|------|----------|---------|
| bool \| null | no | `false` |

#### `sync_by_archives`

If `true`, the node syncs by downloading block archives. Faster than block-by-block sync but requires peers that serve archives.

| Type | Required | Default |
|------|----------|---------|
| bool | no | `true` |

#### `skip_saving_persistent_states`

If `true`, the node skips saving periodic shard state snapshots. Saves disk space but makes recovery after failures harder.

| Type | Required | Default |
|------|----------|---------|
| bool | no | `false` |

#### `states_cache_mode`

Shard state caching strategy.

| Type | Required | Default |
|------|----------|---------|
| string (enum) | no | `"Moderate"` |

| Value | Description |
|-------|-------------|
| `"Off"` | States are saved synchronously and not cached |
| `"Moderate"` | States are saved asynchronously (recommended) |

#### `accelerated_consensus_disabled`

If `true`, use the standard (slower) consensus procedure instead of the accelerated one.

| Type | Required | Default |
|------|----------|---------|
| bool | no | `false` |

#### `validation_countdown_mode`

Controls the countdown behavior for block validation.

| Type | Required | Default |
|------|----------|---------|
| string \| null | no | `null` (equivalent to `"always"`) |

| Value | Description |
|-------|-------------|
| `"always"` | Countdown applies to all blocks |
| `"except-zerostate"` | Countdown does not apply to the zero state |

#### `default_rldp_roundtrip_ms`

Initial RTT (round-trip time) estimate for the RLDP protocol in milliseconds. RLDP (Reliable Large Datagram Protocol) is a reliable transport layer on top of ADNL used for large data transfers.

| Type | Required | Default |
|------|----------|---------|
| u32 \| null | no | `null` (uses the RLDP implementation default) |

#### `unsafe_catchain_patches_path`

Path to a directory with catchain JSON patch files. Used for emergency intervention in the catchain protocol (resync, rotation). **Only for emergency situations.**

| Type | Required | Default |
|------|----------|---------|
| string \| null | no | `null` |

---

### `adnl_node`

ADNL (Abstract Datagram Network Layer) is the core networking protocol of TON. This section defines how the node participates in the ADNL network.

#### `adnl_node.ip_address`

The node's external IP address and UDP port. Must be reachable from the internet. Other nodes connect to this address.

| Type | Required | Format |
|------|----------|--------|
| string | yes | `"IP:PORT"` |

**Important:** The IP must match the external IP assigned to this replica's LoadBalancer service, and the port must match `ports.adnl` in Helm values.

#### `adnl_node.keys`

Cryptographic keys for different node functions. Each key has a `tag` that determines its purpose.

| Field | Type | Description |
|-------|------|-------------|
| `tag` | integer | Key purpose (see below) |
| `data.type_id` | integer | Key type. `1209251014` = Ed25519 |
| `data.pvt_key` | string (base64) | 256-bit Ed25519 private key |

**Key tags:**

| Tag | Purpose |
|-----|---------|
| `1` | DHT key — used for peer discovery |
| `2` | Overlay key — used for block and data exchange |

#### Additional `adnl_node` fields

These optional fields are not commonly needed:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `recv_pipeline_pool` | u8 \| null | `null` | Percentage of CPU cores for packet receive workers |
| `recv_priority_pool` | u8 \| null | `null` | Percentage of workers for priority receive |
| `throughput` | u32 \| null | `null` | Max send throughput (bytes/sec) |
| `timeout_expire_queued_packet_sec` | u32 \| null | `null` | Timeout for queued packets (seconds) |

---

### `gc`

Garbage collector settings for cleaning up stale data.

#### `gc.enable_for_archives`

Enable automatic cleanup of old block archives. If `false`, archives accumulate indefinitely.

| Type | Default |
|------|---------|
| bool | `false` |

#### `gc.archives_life_time_hours`

Archive retention time in hours. Archives older than this are deleted.

| Type | Default |
|------|---------|
| u32 \| null | `null` (delete as soon as possible) |

Example: `48` keeps archives for the last 2 days.

#### `gc.enable_for_shard_state_persistent`

Enable cleanup of old persistent shard state snapshots.

| Type | Default |
|------|---------|
| bool | `false` |

#### `gc.cells_gc_config`

GC settings for cells — the basic data storage units in TON.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `gc_interval_sec` | u32 | `900` (15 min) | How often cell GC runs |
| `cells_lifetime_sec` | u64 | `86400` (24 hours) | Cells unused for longer than this are eligible for deletion |

This lifetime only affects cells used for serving external queries (e.g. when a lite-client requests an account state). The node keeps states it needs for its own operation regardless of this setting. For validators or nodes without a liteserver, you can set a significantly lower value (e.g. `1800` = 30 min) to reduce disk usage and improve performance.

---

### `cells_db_config`

Cell database tuning. If omitted entirely, the node uses built-in defaults for all fields. **If the section is present, all fields must be specified** — partial configs will fail to deserialize.

#### `cells_db_config.states_db_queue_len`

Maximum queue length for state write operations. Controls backpressure during async saves.

| Type | Default |
|------|---------|
| u32 | `1000` |

#### `cells_db_config.prefill_cells_counters`

If `true`, pre-fill the cell counter cache on startup. This loads all cell reference counters into memory, which allows state saves to complete without any disk reads — theoretically very fast. However, this consumes a large amount of RAM and significantly increases startup time.

**Always set to `false` unless you have a specific, well-understood reason to enable it.** It is not required for normal full node or validator operation.

| Type | Default |
|------|---------|
| bool | `false` |

#### `cells_db_config.cells_cache_size_bytes`

Cell cache size in bytes. Larger cache = fewer disk reads. ~4 GB is the standard value.

| Type | Default |
|------|---------|
| u64 | `4000000000` (~4 GB) |

#### `cells_db_config.counters_cache_size_bytes`

Cell reference counter cache size in bytes. Used for reference counting during GC.

| Type | Default |
|------|---------|
| u64 | `4000000000` (~4 GB) |

---

### `collator_config`

Collator configuration — the component that assembles new blocks. Only relevant for validators.

#### `collator_config.cutoff_timeout_ms`

Soft collation timeout in milliseconds. After this time the collator stops adding new transactions and starts finalizing the block.

| Type | Default |
|------|---------|
| u32 | `1000` (1 second) |

#### `collator_config.stop_timeout_ms`

Hard collation timeout. Collation is forcefully stopped after this time. Must be >= `cutoff_timeout_ms`.

| Type | Default |
|------|---------|
| u32 | `1500` (1.5 seconds) |

#### `collator_config.clean_timeout_percentage_points`

Cleanup timeout as a fraction of `cutoff_timeout`. Measured in per-mille (out of 1000). `150` = 15% of cutoff_timeout — time allocated for removing processed messages during collation.

| Type | Default |
|------|---------|
| u32 | `150` |

#### `collator_config.optimistic_clean_percentage_points`

Fraction of `clean_timeout` used for the first cleanup attempt. `1000` = 100%.

| Type | Default |
|------|---------|
| u32 | `1000` |

#### `collator_config.max_secondary_clean_timeout_percentage_points`

Maximum secondary cleanup timeout as a fraction of `cutoff_timeout`. `350` = 35%.

| Type | Default |
|------|---------|
| u32 | `350` |

#### `collator_config.max_collate_threads`

Maximum number of parallel collation threads.

| Type | Default |
|------|---------|
| u32 | `10` |

#### `collator_config.retry_if_empty`

If `true`, retry collation when the resulting block is empty (no transactions). For standard TON consensus, `false` is recommended. Setting `true` can be used in experimental networks to reduce block frequency during low activity.

| Type | Default |
|------|---------|
| bool | `false` |

#### `collator_config.finalize_empty_after_ms`

Time to wait for transactions before finalizing an empty block (ms). If no transactions appear within this time, the block is finalized empty.

| Type | Default |
|------|---------|
| u32 | `800` |

#### `collator_config.empty_collation_sleep_ms`

Pause between collation attempts when there are no transactions (ms). Reduces CPU usage during low activity.

| Type | Default |
|------|---------|
| u32 | `100` |

#### `collator_config.external_messages_timeout_percentage_points`

Fraction of `cutoff_timeout` allocated for processing external messages. `100` = 10%.

| Type | Default |
|------|---------|
| u32 | `100` |

#### `collator_config.external_messages_maximum_queue_length`

Maximum external message queue length. Limits memory usage during external message spam.

| Type | Default |
|------|---------|
| u32 \| null | `25600` |

#### `collator_config_mc`

Separate collator config for the masterchain. Same format as `collator_config`. If not set, `collator_config` is used for all chains.

| Type | Default |
|------|---------|
| object \| null | `null` |

---

### `control_server`

Administrative interface for managing the node — validator key rotation, monitoring, etc. Required for validators; optional for fullnodes.

#### `control_server.address`

Address and port to listen on.

| Type | Format |
|------|--------|
| string | `"IP:PORT"` |

Use `"0.0.0.0:<port>"` to listen on all interfaces. For security, consider `"127.0.0.1:<port>"` if management is local only. The port must match `ports.control` in Helm values.

#### `control_server.server_key`

Server private key for ADNL encryption.

| Field | Type | Description |
|-------|------|-------------|
| `type_id` | integer | `1209251014` = Ed25519 |
| `pvt_key` | string (base64) | 256-bit private key |

#### `control_server.clients`

Authorized clients allowed to connect.

| Field | Type | Description |
|-------|------|-------------|
| `list` | array | Array of client public keys |
| `list[].type_id` | integer | `1209251014` = Ed25519 |
| `list[].pub_key` | string (base64) | Client public key |

If `clients` is omitted or empty, any client can connect.

---

### `lite_server`

Allows lite clients (tonlib, etc.) to connect to the node for queries. The corresponding public key is published in the global config's `liteservers` section for clients to discover it.

#### `lite_server.address`

Address and port for lite client connections. The port must match `ports.liteserver` in Helm values.

| Type | Format |
|------|--------|
| string | `"IP:PORT"` |

#### `lite_server.server_key`

Server private key. Same format as `control_server.server_key`.

#### `lite_server.max_parallel_fast_queries`

Maximum number of concurrent "fast" queries (queries that read from cache or perform simple lookups). Limits concurrency to prevent resource exhaustion under high load.

| Type | Required | Default |
|------|----------|---------|
| u64 \| null | no | `256` |

#### `lite_server.max_parallel_slow_queries`

Maximum number of concurrent "slow" queries (queries that require disk reads, state traversal, or proof generation). These are more resource-intensive, so the default is significantly lower.

| Type | Required | Default |
|------|----------|---------|
| u64 \| null | no | `16` |

#### `lite_server.account_state_cache_size_mb`

Size of the in-memory cache for account states in megabytes. Caches recently queried account states to avoid repeated disk lookups.

| Type | Required | Default |
|------|----------|---------|
| u64 \| null | no | `256` (MB) |

---

### `json_rpc_server`

HTTP JSON-RPC server for API requests.

#### `json_rpc_server.address`

Address and port for the HTTP API. The port must match `ports.jsonRpc` in Helm values.

| Type | Format |
|------|--------|
| string | `"IP:PORT"` |

---

### `metrics`

Prometheus metrics and Kubernetes health probe HTTP server. When present, the node starts an HTTP server with three endpoints:

| Endpoint | Purpose |
|----------|---------|
| `GET /metrics` | Prometheus scrape endpoint |
| `GET /healthz` | Kubernetes liveness probe |
| `GET /readyz` | Kubernetes readiness probe |

If the `metrics` section is absent from the config, the metrics server is **not started** — no metrics, no probes.

For the full metrics catalog see [metrics.md](metrics.md).

#### `metrics.address`

Address and port for the metrics/probes HTTP server. The port must match `ports.metrics` in Helm values.

| Type | Required | Format |
|------|----------|--------|
| string | yes | `"IP:PORT"` |

Recommended: `"0.0.0.0:9100"`.

#### `metrics.histogram_buckets`

Custom histogram bucket boundaries, keyed by metric name suffix. If a key matches the end of a histogram metric name, those buckets are used.

| Type | Required | Default |
|------|----------|---------|
| map\<string, float[]\> | no | `{}` (default duration buckets applied to all `*_seconds` metrics) |

When empty (or absent), the following default buckets are applied to all histograms whose name ends with `seconds`:

```
[0.000001, 0.0001, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 60.0, 120.0, 300.0, 600.0, 3600.0]
```

Example — override buckets for all `*_seconds` histograms and add custom ones for `gas_used`:

```json
"histogram_buckets": {
  "seconds": [0.001, 0.01, 0.1, 0.5, 1.0, 5.0, 30.0, 60.0],
  "gas_used": [1000, 10000, 100000, 500000, 1000000]
}
```

#### `metrics.global_labels`

Key-value pairs added to every metric. Useful for distinguishing nodes when multiple instances report to the same Prometheus.

| Type | Required | Default |
|------|----------|---------|
| map\<string, string\> | no | `{}` |

> **Note:** The bundled [Grafana dashboard](../../../grafana/) uses `network` and `node_id` labels for filtering. If `global_labels` is missing or empty, the dashboard variables will be empty and panels will show no data. Always set both labels.

Example:

```json
"global_labels": {
  "network": "mainnet",
  "node_id": "validator-01"
}
```

#### Full example

```json
"metrics": {
  "address": "0.0.0.0:9100",
  "histogram_buckets": {},
  "global_labels": {
    "network": "mainnet",
    "node_id": "validator-01"
  }
}
```

---

### `extensions`

Optional network extensions.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `disable_broadcast_retransmit` | bool | `false` | Disable broadcast retransmission. Reduces traffic but hurts data propagation |
| `adnl_compression` | bool | `false` | Enable ADNL packet compression. **Not compatible with C++ TON nodes** — only enable in networks where all peers run the Rust node |
| `broadcast_hops` | u8 \| null | `null` | Maximum number of broadcast hops |

---

### `secrets_vault_config`

External secrets vault for storing private keys outside of `config.json`. When configured, the node stores private keys (ADNL, control server, liteserver) in an encrypted vault file instead of plaintext in `config.json`.

This is the same vault used by [nodectl](../../nodectl/README.md) — both the node and nodectl share the same vault format and encryption. The difference is how the vault URL is provided:

| Component | How vault is configured | HashiCorp Vault support |
|-----------|------------------------|------------------------|
| **Node** | `secrets_vault_config` in `config.json` | Planned (next release) |
| **nodectl** | `VAULT_URL` environment variable | Planned (next release) |

Currently only the file-based backend is available:

```json
"secrets_vault_config": {
  "url": "file:///path/to/vault.json&master_key=<64-char-hex>"
}
```

The master key is a 32-byte AES-256 encryption key (64 hex characters). Store it securely — anyone with the key can decrypt the vault file.

| Type | Default |
|------|---------|
| object \| null | `null` |

> **Note:** When `secrets_vault_config` is `null` (default), private keys remain in `config.json` as plaintext. This is acceptable for fullnodes and liteservers. For validators, configuring a vault is recommended.

---

### Auto-managed fields

The following fields are written by the node itself (via the control server) and should not be edited by hand:

| Field | Description |
|-------|-------------|
| `validator_keys` | Validator key records with election IDs and expiration timestamps |
| `validator_key_ring` | Private key storage for validator keys |

In practice these fields are only relevant for testing. During validator elections the node rotates keys and writes them back to `config.json`, but in a Kubernetes environment this is a no-op: the config lives in a Secret, and any pod restart or Helm upgrade overwrites it. You can treat these fields as either immutable or initialization-only — the node will regenerate them as needed via the control server.

---

## Validator-specific sections

> Validator election management is handled by [nodectl](../../nodectl/docs/setup.md) (alpha). The sections below cover node-level config needed for validation.

The `collator_config` section controls block assembly (timeouts, threading, message queue limits). See the [field reference](#collator_config) and the [validator example](#minimal-example-validator) for typical values.

---

## Advanced fields

The following fields exist in the config but are not needed for normal operation. You can safely ignore them unless instructed otherwise.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `accelerated_consensus_disabled` | bool | `false` | Disables accelerated consensus, falling back to the standard (slower) procedure |
| `validation_countdown_mode` | string \| null | `null` (`"always"`) | Countdown mode for validation: `"always"` or `"except-zerostate"` |
| `default_rldp_roundtrip_ms` | u32 \| null | `null` | Initial RTT estimate for the RLDP protocol (ms) |
| `unsafe_catchain_patches_path` | string \| null | `null` | Path to catchain emergency patch files. **Only for emergency situations** |


