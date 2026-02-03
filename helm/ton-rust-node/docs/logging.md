# Logging configuration

TON Node uses [log4rs](https://docs.rs/log4rs/1.3.0/log4rs/) for logging. The config is a YAML file passed to the node via the `log_config_name` field in the node config (`config.json`). In this chart it is mounted as `/main/logs.config.yml`.

A sensible default is bundled with the chart at [files/logs.config.yml](../files/logs.config.yml) and used automatically if no override is given. You can override it inline, via `--set-file logsConfig=path`, or by pointing to an existing ConfigMap (see README for details).

## Table of contents

- [Hot reload](#hot-reload)
- [Appenders](#appenders)
- [Encoder (log format)](#encoder-log-format)
- [Loggers](#loggers)

## Hot reload

The `refresh_rate` field tells log4rs to re-read the config file periodically. This lets you change log levels **without restarting the node** — changes are picked up within the specified interval.

```yaml
refresh_rate: 30 seconds
```

Supported units: `seconds`, `minutes`, `hours`. If omitted, the config is only read once at startup.

This is useful for debugging in production: temporarily raise a logger's level to `debug`, observe the output, then revert — all without restarting the node.

## Appenders

Appenders define **where** logs are written. Each appender has a unique name (the YAML key) and a `kind`. Three kinds are available: `rolling_file`, `console`, and `file`.

Keep in mind that a TON node can produce a **very large volume of logs** — especially during sync, elections, and catch-up. Choose your appender accordingly and pay close attention to log levels (see [available targets](#available-logger-targets) below).

### rolling_file (default, recommended)

Writes to a file with automatic size-based rotation. The chart creates a dedicated `logs` PVC for this. This is the safest choice for production: logs are always available locally and rotation prevents disk exhaustion.

```yaml
appenders:
  rolling_logfile:
    kind: rolling_file
    path: /logs/output.log
    encoder:
      pattern: "{d(%Y-%m-%d %H:%M:%S.%f)} {l} [{t}] {I}: {m}{n}"
    policy:
      kind: compound
      trigger:
        kind: size
        limit: 25 gb
      roller:
        kind: fixed_window
        pattern: '/logs/output_{}.log'
        base: 1
        count: 4
```

The `policy` section controls when and how rotation happens:

**Trigger: `size`** — rotate when the file reaches the given size.

| Field | Description |
|-------|-------------|
| `limit` | Max file size. Supports suffixes: `b`, `kb`, `mb`, `gb`, `tb` (e.g. `25 gb`) |

**Roller: `fixed_window`** — renames old files using a pattern with a sliding index.

| Field | Required | Description |
|-------|----------|-------------|
| `pattern` | yes | Archive filename template. `{}` is replaced by the index. Append `.gz` to compress archives |
| `base` | no (default `0`) | Starting index |
| `count` | yes | Maximum number of archive files |

Example with `pattern: '/logs/output_{}.log'`, `base: 1`, `count: 4`:
on rotation `output.log` becomes `output_1.log`, the old `output_1.log` shifts to `output_2.log`, and so on up to `output_4.log` which is deleted.

**Tip:** Use `.gz` in the pattern to compress archives — they shrink roughly 5-10x:

```yaml
pattern: '/logs/output_{}.log.gz'
```

**Storage sizing:** The Helm value `storage.logs.size` controls the PVC size for `/logs`. Make sure your rotation settings fit within it. For example, the default config uses 25 GB per file with 4 rotations — that's up to 125 GB peak (1 active + 4 archives). The default `storage.logs.size` is `150Gi`, leaving headroom. If you reduce rotation limits (e.g. 1 GB x 10 archives with `.gz`), real disk usage drops to ~3-5 GB and you can safely reduce the volume size.

### console

Writes to stdout or stderr. Use this if your cluster runs a log collection stack (Loki, Fluentd, Elasticsearch, etc.) and you want the collector to handle storage. Be very careful with log levels — at `debug`/`trace` the node can easily saturate any collector. If you switch to console-only logging, disable the logs volume by setting `storage.logs.enabled` to `false`.

```yaml
appenders:
  stdout:
    kind: console
    target: stdout          # or "stderr"
    encoder:
      pattern: "{d(%Y-%m-%d %H:%M:%S.%f)} {l} [{t}] {I}: {m}{n}"
```

### file

Writes to a file without rotation. Not recommended — the file grows indefinitely. Use `rolling_file` instead.

```yaml
appenders:
  logfile:
    kind: file
    path: /logs/output.log
    append: true            # default: true
    encoder:
      pattern: "..."
```

### Filters

Filters can be added to any appender for additional message filtering.

**Threshold filter** — drops messages below the specified level:

```yaml
appenders:
  stdout:
    kind: console
    filters:
      - kind: threshold
        level: warn
    encoder:
      pattern: "..."
```

## Encoder (log format)

All appenders use an `encoder` to format each log line. The default kind is `pattern`:

```yaml
encoder:
  pattern: "{d(%Y-%m-%d %H:%M:%S.%f)} {l} [{t}] {I}: {m}{n}"
```

### Format specifiers

| Specifier | Name | Description |
|-----------|------|-------------|
| `{d}` / `{d(fmt)}` | date | Timestamp. Default is ISO 8601. Custom format uses chrono syntax: `{d(%Y-%m-%d %H:%M:%S.%f)}` |
| `{l}` | level | Log level: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE` |
| `{m}` | message | Log message body |
| `{n}` | newline | Platform-dependent newline |
| `{t}` | target | Logger target (module name or explicit `target:` in the log macro) |
| `{I}` | thread_id | Numeric thread ID |
| `{T}` | thread | Thread name |
| `{f}` | file | Source file name |
| `{L}` | line | Source line number |
| `{M}` | module | Module path |
| `{P}` | pid | Process ID |
| `{h(..)}` | highlight | Colorizes the inner text by level (only useful in console, not in files) |

### Example output

```
2025-01-15 14:30:45.123456 INFO [validator] 140234567890: Block validated successfully
```

> **Note:** Avoid `{h(...)}` in file appenders — it writes ANSI escape codes into the log which make it harder to read and grep.

## Loggers

### Root logger

The root logger is the default — all messages not captured by a named logger are handled here.

```yaml
root:
  level: error
  appenders:
    - rolling_logfile
```

| Field | Required | Description |
|-------|----------|-------------|
| `level` | yes | Log level: `off`, `error`, `warn`, `info`, `debug`, `trace` |
| `appenders` | yes | List of appender names to write to |

### Named loggers

Named loggers set different log levels for different components. The name corresponds to the `target` used in the node's code.

```yaml
loggers:
  validator:
    level: info
```

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `level` | no | inherited from parent | Log level |
| `appenders` | no | `[]` | Appenders for this logger |
| `additive` | no | `true` | If `true`, messages also propagate to the parent logger's appenders (root) |

Loggers form a hierarchy via `::`. For example, `node::network` is a child of `node`. With `additive: true` (default), messages from `node::network` go to both `node`'s appenders and root's.

### Log levels (most verbose to least)

| Level | Description |
|-------|-------------|
| `trace` | Most detailed. Execution flow tracing |
| `debug` | Debug information |
| `info` | Normal operation messages |
| `warn` | Potential problems |
| `error` | Errors that don't stop the node |
| `off` | Logging completely disabled |

### Available logger targets

These are the targets you can configure in the `loggers` section:

| Target | Description |
|--------|-------------|
| `node` | Core node messages |
| `boot` | Node bootstrap and initialization |
| `sync` | Block synchronization |
| `node::network` | Node networking |
| `node::network::neighbours` | Neighbour tracking (very noisy) |
| `node::network::liteserver` | Liteserver request handling |
| `node::validator::collator` | Block collation |
| `adnl` | ADNL network protocol |
| `overlay` | Overlay networks |
| `rldp` | RLDP protocol (reliable large datagrams) |
| `dht` | Distributed Hash Table |
| `ton_block` | Block parsing and serialization |
| `executor` | Transaction execution |
| `tvm` | TON Virtual Machine |
| `validator` | Validation (general) |
| `validator_manager` | Validator management |
| `catchain` | Catchain consensus protocol |
| `catchain_adnl_overlay` | ADNL overlay for catchain |
| `validator_session` | Validator sessions |
| `validate_query` | Individual block/query validation |
| `consensus_common` | Common consensus logic |
| `storage` | Data storage |
| `index` | Data indexing |
| `ext_messages` | External message handling |
| `telemetry` | Telemetry and metrics |
