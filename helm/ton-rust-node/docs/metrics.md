# Metrics reference

The TON node exposes 51 Prometheus metrics across 8 subsystems. All metrics are served at `GET /metrics` on the metrics port (default `9100`) in standard Prometheus text format.

## Table of contents

- [Configuration](#configuration)
- [Build info](#build-info)
- [Naming convention](#naming-convention)
- [Metric types](#metric-types)
- [Labels](#labels)
- [engine (8 metrics)](#engine)
- [validator (8 metrics)](#validator)
- [collator (14 metrics)](#collator)
- [outqueue (4 metrics)](#outqueue)
- [ext_messages (2 metrics)](#ext_messages)
- [network (7 metrics)](#network)
- [db (5 metrics)](#db)
- [block (3 metrics)](#block)
## Configuration

Metrics are configured in the node's `config.json`:

```json
{
  "metrics": {
    "address": "0.0.0.0:9100",
    "histogram_buckets": {},
    "global_labels": {
      "network": "mainnet",
      "node_id": "validator-01"
    }
  }
}
```

| Field | Description |
|-------|-------------|
| `address` | Listen address for the metrics/probes HTTP server. If the `metrics` section is absent, the server is not started. |
| `histogram_buckets` | Custom histogram buckets by metric name. When empty, default duration buckets are applied to all `*_seconds` histograms via suffix matcher. |
| `global_labels` | Key-value pairs added to every metric. Useful for distinguishing nodes in a shared Prometheus. |

Default histogram buckets for `*_seconds` metrics:

```
[0.000001, 0.0001, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 60.0, 120.0, 300.0, 600.0, 3600.0]
```

## Build info

`ton_node_build_info` is an informational gauge always set to `1`. Build metadata is encoded as labels:

```
ton_node_build_info{version="0.1.0", commit="a1b2c3d...", branch="master", build_time="2025-01-15 12:00:00 +0000", rustversion="rustc 1.82.0", arch="aarch64", os="linux"} 1
```

| Label | Description |
|-------|-------------|
| `version` | Package version from `Cargo.toml` |
| `commit` | Full git commit hash |
| `branch` | Git branch at build time |
| `build_time` | Build timestamp |
| `rustversion` | Rust compiler version |
| `arch` | Target architecture (`x86_64`, `aarch64`, etc.) |
| `os` | Target OS (`linux`, `macos`, etc.) |

## Naming convention

All metrics follow the format:

```
ton_node_{subsystem}_{metric_name}[_{unit_suffix}]
```

- `ton_node_` — application prefix, avoids collisions in shared Prometheus instances
- `{subsystem}` — functional area (see table below)
- `{metric_name}` — snake_case, descriptive name
- `{unit_suffix}` — optional: `_total` (counters), `_seconds` (durations), `_bytes` (sizes), `_ratio` (dimensionless)

| Subsystem | Scope |
|-----------|-------|
| `engine` | Core node state, sync status, masterchain tracking |
| `validator` | Block validation lifecycle |
| `collator` | Block collation lifecycle |
| `outqueue` | Outbound message queue cleanup |
| `ext_messages` | External messages queue |
| `network` | ADNL, catchain, overlay, neighbour stats |
| `db` | Database, shard state, GC, persistent state |
| `block` | Block parsing, block sizes |

## Metric types

| Type | Behavior | Suffix convention |
|------|----------|-------------------|
| **counter** | Monotonically increasing. Resets to 0 on restart. | `_total` |
| **gauge** | Arbitrary value, can go up and down. | (none), `_seconds`, `_ratio` |
| **histogram** | Samples sorted into configurable buckets. Exposes `_bucket`, `_sum`, `_count`. | `_seconds`, `_bytes`, `_ratio`, or (none) |

## Labels

Some metrics carry labels for additional dimensions:

| Label | Subsystems | Values | Description |
|-------|------------|--------|-------------|
| `shard` | validator, collator, outqueue | `ShardIdent` string | Shard identifier for per-shard breakdown |
| `step` | outqueue | step identifier | Outqueue clean operation step |
| `peer` | network | peer key ID (hex) | ADNL peer for roundtrip measurements |
| `neighbour` | network | neighbour key ID (hex) | Overlay neighbour for reliability tracking |

---

## engine

Core node state: sync progress, masterchain tracking, validation intent, applied transactions.

**8 metrics** (7 gauges, 1 counter).

| Metric | Type | Description |
|--------|------|-------------|
| `ton_node_engine_sync_status` | gauge | Sync state machine value |
| `ton_node_engine_timediff_seconds` | gauge | Seconds between now and last applied masterchain block |
| `ton_node_engine_last_mc_block_utime` | gauge | Unix timestamp of last applied masterchain block |
| `ton_node_engine_last_mc_block_seqno` | gauge | Seqno of last applied masterchain block |
| `ton_node_engine_shards_mc_seqno` | gauge | MC block seqno last processed by shard client |
| `ton_node_engine_shards_timediff_seconds` | gauge | Seconds between now and MC block last processed by shard client |
| `ton_node_engine_will_validate` | gauge | 1 if node intends to validate, 0 otherwise |
| `ton_node_engine_applied_transactions_total` | counter | Total transactions in all applied blocks (MC + shards) |

### Sync status values

| Value | State | Description |
|-------|-------|-------------|
| 0 | `not_set` | Initial state, status not yet determined |
| 1 | `start_boot` | Boot sequence started |
| 3 | `load_states` | Loading shard states from network |
| 4 | `finish_boot` | Boot sequence completing |
| 5 | `sync_blocks` | Syncing blocks from network |
| 6 | `synced` | Fully synced with the network |
| 7 | `checking_db` | Database integrity check in progress |
| 8 | `db_broken` | Database corruption detected |

A healthy, synced node reports `sync_status = 6`. During initial sync or restart, the value progresses through `1 → 3 → 4 → 5 → 6`.

### Timediff

`timediff_seconds` measures the gap between wall-clock time and `gen_utime` of the last applied masterchain block. Values under 20 s indicate a healthy node; above 60 s warrants investigation.

---

## validator

Block validation lifecycle: state, set membership, outcomes, and gas.

**8 metrics** (4 gauges, 3 counters, 1 histogram).

| Metric | Type | Description |
|--------|------|-------------|
| `ton_node_validator_status` | gauge | Validation state machine value |
| `ton_node_validator_in_current_set` | gauge | 1 if node is in current validator set (p34) |
| `ton_node_validator_in_next_set` | gauge | 1 if node is in next validator set (p36) |
| `ton_node_validator_active` | gauge | Number of validation queries currently running |
| `ton_node_validator_successes_total` | counter | Successful block validations |
| `ton_node_validator_failures_total` | counter | Failed block validations |
| `ton_node_validator_ref_block_failures_total` | counter | Failed reference shard block validations |
| `ton_node_validator_gas_rate_ratio` | histogram | Gas rate ratio from validation |

### Validation status values

| Value | State | Description |
|-------|-------|-------------|
| 0 | `Not in Set` | Node is not in the current validator set |
| 1 | `Waiting` | Waiting for next validation round |
| 2 | `Countdown` | Elected, countdown to validation start |
| 3 | `Active` | Actively validating blocks |

---

## collator

Block collation lifecycle: outcomes, duration, gas usage, message flow, and transaction counts.

**14 metrics** (2 gauges, 6 counters, 6 histograms).

| Metric | Type | Description |
|--------|------|-------------|
| `ton_node_collator_active` | gauge | Number of collation queries currently running |
| `ton_node_collator_successes_total` | counter | Successful block collations |
| `ton_node_collator_failures_total` | counter | Failed block collations |
| `ton_node_collator_duration_seconds` | histogram | Block collation duration (end-to-end) |
| `ton_node_collator_process_ext_messages_seconds` | histogram | Time to process inbound external messages |
| `ton_node_collator_process_new_messages_seconds` | histogram | Time to process new (internal) messages |
| `ton_node_collator_gas_used` | histogram | Gas consumed per collated block |
| `ton_node_collator_gas_rate_ratio` | histogram | Gas rate ratio from collation |
| `ton_node_collator_dequeued_messages_total` | counter | Messages dequeued from outbound queue during collation |
| `ton_node_collator_enqueued_messages_total` | counter | Messages enqueued to outbound queue during collation |
| `ton_node_collator_inbound_messages_total` | counter | Inbound messages processed during collation |
| `ton_node_collator_outbound_messages_total` | counter | Outbound messages produced during collation |
| `ton_node_collator_transit_messages_total` | counter | Transit messages (forwarded between shards) during collation |
| `ton_node_collator_executed_transactions_total` | counter | Transactions executed during collation |

---

## outqueue

Outbound message queue: periodic cleanup stats.

**4 metrics** (all gauges).

| Metric | Type | Description |
|--------|------|-------------|
| `ton_node_outqueue_clean_partial` | gauge | 1 if last outqueue clean was partial (not all messages processed) |
| `ton_node_outqueue_clean_duration_seconds` | gauge | Duration of last outqueue clean in seconds |
| `ton_node_outqueue_clean_processed` | gauge | Messages processed in last outqueue clean |
| `ton_node_outqueue_clean_deleted` | gauge | Messages deleted in last outqueue clean |

---

## ext_messages

External messages queue: messages received from clients awaiting inclusion in blocks.

**2 metrics** (1 gauge, 1 counter).

| Metric | Type | Description |
|--------|------|-------------|
| `ton_node_ext_messages_queue_size` | gauge | Current external messages queue size |
| `ton_node_ext_messages_expired_total` | counter | Expired external messages removed from queue |

---

## network

Networking: ADNL roundtrip, catchain timings, overlay queries, neighbour reliability.

**7 metrics** (1 gauge, 1 counter, 5 histograms).

| Metric | Type | Labels | Description |
|--------|------|--------|-------------|
| `ton_node_network_adnl_roundtrip_seconds` | histogram | `peer` | ADNL query roundtrip time |
| `ton_node_network_catchain_overlay_query_seconds` | histogram | | Catchain overlay query time |
| `ton_node_network_catchain_send_seconds` | histogram | | Catchain send message time |
| `ton_node_network_catchain_client_query_seconds` | histogram | | Catchain client query time |
| `ton_node_network_consensus_overlay_query_seconds` | histogram | | Consensus overlay query time |
| `ton_node_network_neighbour_failures_total` | counter | `neighbour` | Failed queries to overlay neighbours |
| `ton_node_network_neighbour_unreliability` | gauge | `neighbour` | Neighbour unreliability score (higher = less reliable) |

---

## db

Database operations: shard state management, GC, persistent state, Merkle updates.

**5 metrics** (1 gauge, 4 histograms).

| Metric | Type | Description |
|--------|------|-------------|
| `ton_node_db_shardstate_queue_size` | gauge | Shard state processing queue size |
| `ton_node_db_shardstate_gc_seconds` | histogram | Shard state garbage collection duration |
| `ton_node_db_persistent_state_write_seconds` | histogram | Persistent state write (BOC serialization) duration |
| `ton_node_db_restore_merkle_update_seconds` | histogram | Merkle update duration during chain restore |
| `ton_node_db_calc_merkle_update_seconds` | histogram | Merkle update calculation duration |

---

## block

Block parsing and size metrics.

**3 metrics** (all histograms).

| Metric | Type | Description |
|--------|------|-------------|
| `ton_node_block_accounts_parsing_seconds` | histogram | Time to parse accounts from a block |
| `ton_node_block_parsed_accounts` | histogram | Number of accounts parsed per block |
| `ton_node_block_size_bytes` | histogram | Block size in bytes |
