# Audit Log

## What it is

nodectl writes a structured, append-only log of domain events — elections, config
mutations, authentication, vault operations — to a newline-delimited JSON file
(`audit.jsonl`).

The audit log is **separate from the `tracing` service log** (stderr / journald).
Use the table below to decide where to look.

| Use case | Where |
|---|---|
| Debugging service internals, stack traces | tracing logs (`RUST_LOG`) |
| HTTP access / request logs | *(not implemented; would be tracing spans)* |
| Metrics / counters | Prometheus *(future)* |
| Domain events: who did what and when | **audit log** |

## Out of scope

- Per-RPC / per-request logging
- Metrics / dashboards
- Debug noise (heartbeats, cache refreshes, routine polls)
- High-frequency sources (> ~10 events/sec)
- Tamper-evidence (hash chain, signed events) — see RFC 9162 for future work

## Event types

Events are grouped by source:

| Prefix | Events |
|---|---|
| `elections.*` | Key generated, stake submitted/accepted/skipped/failed/recovered, withdraw processed/failed |
| `rest_api.*` | Config updated, auth login succeeded/rejected, token rejected |
| `vault.*` | Key created / removed *(producers not wired yet)* |
| `rewards.*` | Distribution started/completed/failed, recipient skipped *(producers not wired yet)* |
| `system.*` | Service started/stopped, audit events dropped |

Each event contains:

- `id` — UUID v7 (sortable by creation time)
- `ts` — RFC3339 timestamp with millisecond precision (`2026-05-22T12:10:30.123Z`)
- `outcome` — `success`, `failure`, or `skipped`
- `event_type` — dotted string (e.g. `elections.stake_submitted`)
- `data` — event-specific payload (omitted when `include_payload = false`)
- `actor` — who triggered the action (`service` task or `user` identity)
- `target` — what the action was applied to (node, config, vault key, …)

## File layout

```
./logs/audit.jsonl       ← current file (line 0 is a file header, not an event)
./logs/audit.jsonl.1     ← most-recent rotation
./logs/audit.jsonl.2
…
./logs/audit.jsonl.9
```

The first line of every file is a **header** (no `event_type` field):

```json
{"schema_version":1,"service":"nodectl","service_version":"0.7.0","host":"validator-1","started_at":"2026-05-22T12:00:00.000Z"}
```

Defaults: 100 MiB per file, 10 files → ~1 GiB total history.

## Configuration

All fields live under the `audit_log` key in the nodectl config file.
None of them require a service restart — the values are read at startup.

| Field | Default | Description |
|---|---|---|
| `enabled` | `true` | Set to `false` to disable the audit log entirely |
| `path` | `./logs/audit.jsonl` | Path to the active log file; rotated files get `.1`…`.N` suffixes |
| `max_size_bytes` | `104857600` (100 MiB) | Rotate when the live file exceeds this size |
| `max_files` | `10` | Number of rotated files to keep (oldest is deleted on overflow) |
| `batch_interval_ms` | `1000` | How often (ms) the writer flushes a batch to disk |
| `batch_max_events` | `100` | Flush early when a batch reaches this many events |
| `queue_capacity` | `10000` | In-memory channel capacity between `record()` callers and the writer task |
| `queue_full_timeout_ms` | `250` | How long (ms) `record()` waits before dropping an event when the queue is full |
| `fsync_on_batch` | `false` | Call `fsync` after every batch — see [Durability](#durability) |
| `include_payload` | `true` | Write `data` fields; set to `false` to log event metadata only |
| `record_client_ip` | `false` | Include client IP in `rest_api.*` events — see [PII](#pii-and-retention) |
| `ip_anonymize` | `false` | Mask last IPv4 octet / last two IPv6 groups when recording IP |
| `ring_buffer_capacity` | `100` | In-memory ring for the REST read-path (see [Where it's consumed](#where-its-consumed)) |

Example minimal override (all other fields keep their defaults):

```json
{
  "audit_log": {
    "path": "/var/log/nodectl/audit.jsonl",
    "max_files": 30,
    "fsync_on_batch": true
  }
}
```

## Durability

With `fsync_on_batch = false` (default), the kernel page cache is flushed on
the OS's own schedule. On a hard kill (`SIGKILL`) or power loss, up to
`batch_interval_ms` (~1 s) of events may be lost.

Set `fsync_on_batch = true` for strict durability at higher disk cost (one
`fsync` per second by default; one per `batch_max_events` events at high
throughput).

Events dropped because the writer queue is full are counted in the
`system.audit_events_dropped` event emitted on the next flush.

## PII and retention

Audit events may contain operator usernames, optionally client IP addresses,
and config change details. In GDPR-style regimes, IP addresses and usernames
are personal data.

- `record_client_ip = false` (default): no IP is ever written.
- `record_client_ip = true`, `ip_anonymize = false`: full IP written.
- `record_client_ip = true`, `ip_anonymize = true`: last IPv4 octet zeroed,
  last two IPv6 groups masked (`::0:0`).

Retention is bounded by `max_size_bytes × max_files`. Tune for your policy.
Log files are **not** automatically deleted after a time-based retention period —
external tooling (logrotate, cron) is needed if you require time-based purges.

## File permissions

On Unix, the live file and all rotated files are created with mode `0600`
(owner read/write only). The directory is not created with any special mode —
ensure the directory itself has appropriate permissions.

Tamper-evidence (hash chains, signed events) is **out of scope** for the
current release. Treat the audit log as protected by host trust and filesystem
ACLs, not by cryptography.

## Where it's consumed

`GET /v1/elections` reads from the **in-memory ring buffer** (last
`ring_buffer_capacity` events, default 100) and enriches `our_participants`
with:

- `stake_submissions` — stake submission history from audit
- `last_error` — latest error-class event (stake skipped, stake failed, withdraw failed)

The JSONL file on disk is **not** parsed on the hot path.

## Analyzing the log

```sh
# Count events by type
jq -r .event_type logs/audit.jsonl | sort | uniq -c | sort -rn

# All events for one election round
jq 'select(.target.election_id == 1779265552)' logs/audit.jsonl

# Failed or skipped stakes in the last file
jq 'select(.outcome == "failure" or .outcome == "skipped")
    | select(.event_type | startswith("elections.stake"))' logs/audit.jsonl

# Config mutations by a specific user
jq 'select(.event_type == "rest_api.config_updated" and .actor.id == "alice")' \
  logs/audit.jsonl

# Tail-follow live events
tail -f logs/audit.jsonl | jq .

# All events in a time range
jq 'select(.ts >= "2026-05-22T10:00:00Z" and .ts < "2026-05-22T11:00:00Z")' \
  logs/audit.jsonl

# Events across rotated files (newest first)
cat logs/audit.jsonl.1 logs/audit.jsonl | jq .
```
