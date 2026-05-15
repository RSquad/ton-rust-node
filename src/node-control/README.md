# nodectl — TON Node Control Tool

**nodectl** is a utility for managing TON nodes. It allows you to interact with the TON blockchain, monitor states, manage node keys, and automatically participate in validator elections.

## Table of Contents

- [Features](#features)
  - [Features Roadmap](#features-roadmap)
- [Architecture](#architecture)
- [Operating Modes](#operating-modes)
  - [CLI](#1-cli)
  - [Daemon](#2-daemon)
- [Building](#building)
- [Global Flags](#global-flags)
- [Commands](#commands)
  - [Configuration Commands](#configuration-commands)
  - [Key Management Commands](#key-management-commands)
  - [Authentication Commands](#authentication-commands)
  - [Deploy Commands](#deploy-commands)
  - [Service Command](#service-command)
  - [Service API Commands](#service-api-commands)
  - [TON HTTP API](#ton-http-api)
  - [Voting Commands](#voting-commands)
- [REST API Endpoints](#rest-api-endpoints)
- [Configuration](#configuration)
  - [Config Structure](#config-structure)
  - [Section Descriptions](#section-descriptions)
  - [Default Config Example](#default-config-example)
- [Service Mode (Daemon)](#service-mode-daemon)
  - [Elections Task](#elections-task)
  - [Logging](#logging)
- [Usage Examples](#usage-examples)
- [Related Setup Guides](#related-setup-guides)

## Features

- **Multi-node** — manage multiple nodes from a single config
- **SecretVault support** — store sensitive data (like wallet keys, control server keys) in a secure vault
- **TON HTTP API** — retrieve blockchain data through TON HTTP API
- **REST API Server** — HTTP API for monitoring, managing elections, and controlling tasks
- **Automatic elections** — automatic participation in validator elections
- **Pool support** — direct staking from the validator wallet, **Single Nominator Pool (SNP)**, or **TONCore nominator** (one or two on-chain pool contracts)
- **Flexible stake policies** — minimum, fixed, or split50 stake strategies with per-node overrides
- **Swagger UI** — interactive API documentation

### Features roadmap

|   Feature   | Status | Comment |
|-------------|------------|-------------|
| Automatic elections | Done | - |
| Nominator pools (SNP + TONCore) | Done | SNP via `config pool add`; TONCore via `config pool add core` |
| Automatic Voting for proposals | Done | - |
| REST API Server | Done | Includes Swagger UI |
| REST API cli commands | Done | - |
| Liquid Staking Pools support | Not implemented | - |

## Architecture

![nodectl Architecture](nodectl-arch.svg)


## Operating Modes

Nodectl can operate in 2 modes.

### 1. CLI

Execute single commands with immediate exit:

```bash
nodectl <cmd>
```

There are several types of commands:

- **TON HTTP API commands** — retrieve blockchain data
- **Configuration commands** — generate and manage config files (nodes, wallets, pools, bindings, elections)
- **Automation** — contracts task (auto-deploy / auto-topup): top-level `nodectl automation` talks to the service API; see **[Contracts automation](./docs/contracts-automation.md)**
- **Key management commands** — generate, import, and manage vault keys
- **Deploy commands** — deploy wallets and nominator pools
- **Service API commands** — interact with a running nodectl daemon

### 2. Daemon

Run as a service for automatic task execution using the `service` subcommand:

```bash
# To run service without secret vault
nodectl service -c config.json
# Or with vault
VAULT_URL=<vault-url-with-params> nodectl service -c config.json
```

In this mode, nodectl runs as a daemon and executes tasks specified in the configuration:

- **Elections Task** — automatic participation in validator elections
- **Voting Task** - automatic voting for proposals
- **REST API Server** — HTTP API for monitoring elections, validators, and controlling tasks

---

## Building

```bash
cargo build --release -p nodectl
```

The binary will be available at `target/release/nodectl`.

---

## Global Flags

| Flag | Description |
|------|-------------|
| `--help` / `-h` | Show help |
| `--version` / `-V` | Show version |

> **Note:** The `--config` (`-c`) flag is specified per subcommand (e.g. `nodectl service -c config.json`, `nodectl config-param -c config.json 34`), not globally. The **`vote`** subcommand is an exception: it also accepts global **`--url` / `-u`**, **`--token`**, and **`--config`** (same resolution rules as `nodectl config` / `nodectl api`).

---

## Commands

To enable detailed logging output, use the RUST_LOG environment variable (available log levels: error, warn, info, debug, trace):
```bash
RUST_LOG=debug nodectl ...
```

### Automation command

REST client for **`/v1/automation/settings`**. Uses the same URL / JWT / `--config` resolution as [`config`](#configuration-commands) (`--url` / `-u`, `NODECTL_URL`, `--token`, `NODECTL_API_TOKEN`, `--config`, `CONFIG_PATH`).

| Subcommand | Purpose |
|------------|---------|
| `ls` | Show settings (`--format table` or `json`) |
| `tick <SEC>` | Contracts task tick interval (seconds) |
| `wallet` | Wallet deploy / top-up / threshold in **TON**: `--deploy`, `--topup`, `--threshold` (at least one required) |
| `pool` | Pool deploy amounts in **TON**: `--deploy` applies to both SNP and TONCore; `--snp` / `--ton-core` override that kind (at least one flag required) |
| `enable deploy` \| `enable topup` | Turn auto-deploy or auto-topup **on** |
| `disable deploy` \| `disable topup` | Turn auto-deploy or auto-topup **off** |

```bash
nodectl automation ls
nodectl automation ls --format json
nodectl automation tick 60
nodectl automation wallet --deploy 1.1 --topup 10 --threshold 5
nodectl automation pool --deploy 1.5
nodectl automation pool --deploy 1.5 --ton-core 2
nodectl automation pool --snp 1.1 --ton-core 2
nodectl automation enable deploy
nodectl automation enable topup
nodectl automation disable deploy
nodectl automation disable topup
```

Full REST and on-disk **`automation`** block: **[Contracts automation](./docs/contracts-automation.md)**.

---

### Configuration Commands

Commands for managing nodectl configuration. **All `config` subcommands except `config generate` are REST clients that require a running nodectl service** — they talk to the daemon over the HTTP API. The top-level **`automation`** command uses the same rules. The service URL is resolved (in order) from `--url` / `-u` (or the `NODECTL_URL` env var), or from `http.bind` inside `--config` (default `nodectl-config.json`, env `CONFIG_PATH`). Protected endpoints require an `operator` JWT token passed via `--token` (or the `NODECTL_API_TOKEN` env var) — see the [Authentication Commands](#authentication-commands) section for how to obtain one.

#### `config generate`

Generate a new default configuration file.

| Flag | Short form | Description |
|------|------------|-------------|
| `--output <FILE>` | `-o` | Output path for the configuration file (default: `nodectl-config.json`) |
| `--force` | `-f` | Overwrite existing file |

```bash
# Generate default config
nodectl config generate

# Generate with custom path
nodectl config generate --output my-config.json

# Overwrite existing file
nodectl config generate --output my-config.json --force
```

See also: [Default Config Example](#default-config-example)

---

#### `config node`

Manage nodes (ADNL control server connections) in the configuration file.

##### `config node add`

Add a node to the configuration. Each node represents a connection to a TON node's Control Server via ADNL protocol.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Node name (unique identifier) |
| `--control-server-endpoint <IP:PORT>` | `-e` | Control server endpoint address |
| `--control-server-pubkey <KEY>` | `-p` | Control server public key (base64) |
| `--control-client-secret-name <NAME>` | `-s` | Vault secret name for ADNL client private key |

```bash
nodectl config node add \
  --name node0 \
  --control-server-endpoint 192.168.1.100:50000 \
  --control-server-pubkey "kGViQgAAAAB..." \
  --control-client-secret-name "node0-client-secret"
```

##### `config node ls`

List all configured nodes.

```bash
nodectl config node ls
# or with json format
nodectl config node ls --format json
```

##### `config node rm`

Remove a node from the configuration.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Node name to remove |

```bash
nodectl config node rm --name node0
```

---

#### `config wallet`

Manage wallets in the configuration file. Wallets are used for validator election submissions and TON transfers.

##### `config wallet add`

Add a wallet to the configuration. The wallet key is stored in the vault and referenced by secret name.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Wallet name (unique identifier) |
| `--secret-name <NAME>` | `-s` | Vault secret name for wallet key |
| `--version <VERSION>` | `-v` | Wallet version: `V3R2`, `V4R2`, `V5R1` (default: `V3R2`) |
| `--subwallet-id <ID>` | `-i` | Subwallet ID (default: `42`) |
| `--workchain <ID>` | `-w` | Workchain ID (default: `-1`) |

```bash
# Add a wallet with specific version and subwallet
nodectl config wallet add \
  --name wallet0 \
  --secret-name "wallet0-key" \
  --version V4R2 \
  --subwallet-id 100 \
  --workchain -1
```

##### `config wallet ls`

List all configured wallets.

```bash
nodectl config wallet ls
# or with json format
nodectl config wallet ls --format json
```

##### `config wallet rm`

Remove a wallet from the configuration.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Wallet name to remove |

```bash
nodectl config wallet rm --name wallet0
```

##### `config wallet send`

Send TON from a configured wallet to an arbitrary address.

| Flag | Short form | Description |
|------|------------|-------------|
| `--from <NAME>` | `-f` | Source wallet name |
| `--to <ADDRESS>` | `-t` | Destination address |
| `--amount <TON>` | `-a` | Amount in TON |
| `--bounce <BOOL>` | `-b` | Bounce transfer to the sender if recipient fails to process it |

```bash
# Send 10 TON from wallet0
nodectl config wallet send \
  --from wallet0 \
  --to "-1:abc123..." \
  --amount 10.0

# Send with bounce disabled (e.g. to a non-deployed address)
nodectl config wallet send \
  --from wallet0 \
  --to "EQDrjaLahLkMB-hMCmkzOyBuHJ186Fl6..." \
  --amount 5.0 \
  --bounce false
```

---

#### `config pool`

Manage nominator pools in the configuration. Pools are stored as tagged JSON: **`kind: "snp"`** or **`kind: "core"`**. Both add variants, `ls`, and `rm` flow through the REST API on the running service.

- **`config pool add`** — **Single Nominator Pool (SNP)** only (`PoolConfig::SNP`: optional `address`, `owner`).
- **`config pool add core`** — **TONCore nominator** (`PoolConfig::TONCore`): up to **two** logical slots (`pools[0]`, `pools[1]`) for even/odd validation rounds. Each slot is configured by a separate command call with explicit **`--even`** or **`--odd`**, and optional `address` and/or deploy `params` (`TonCoreInitParams`) for that slot.
- **`config pool deposit-validator` / `withdraw-validator`** — TONCore-only validator deposit/withdrawal flows that build and send the on-chain message through a configured wallet (require vault access; not a REST client).

##### `config pool add` (SNP)

| Flag | Short | Description |
|------|-------|-------------|
| `--name <NAME>` | `-n` | Pool name (unique identifier) |
| `--address <ADDRESS>` | `-a` | Deployed pool address (raw or base64url) |
| `--owner <ADDRESS>` | `-o` | Owner address (for deployment / verification) |

At least one of `--address` or `--owner` is required.

```bash
nodectl config pool add --name pool0 --address "-1:pool_contract_address"
nodectl config pool add --name pool0 --owner "-1:owner_address"
```

##### `config pool add core` (TONCore)

| Flag | Short | Description |
|------|-------|-------------|
| `--name <NAME>` | `-n` | Pool name (unique identifier) |
| `--validator-share` | | Slot deploy: reward share in **basis points** (`100` = 1%; must be **below 10000** so nominators earn rewards, e.g. `5000` = 50%). Mutually exclusive with `--validator-share-percent`. |
| `--validator-share-percent` | | Same as share above but as a **percent** (`[0, 100)`, not 100 — nominators need a reward share). Decimals allowed (e.g. `50.4` → 5040 bp). |
| `--max-nominators` | | Optional; default from contract defaults |
| `--min-validator-stake` | | Optional; minimum validator stake in **TON** |
| `--min-nominator-stake` | | Optional; minimum nominator stake in **TON** |
| `--address` | | Existing pool address for selected slot |
| `--even` | | Configure slot 0 (even rounds), required unless `--odd` is set |
| `--odd` | | Configure slot 1 (odd rounds), required unless `--even` is set |

At least one of `--address`, `--validator-share`, or `--validator-share-percent` must be set for the selected slot. Use the same pool name with two commands to configure both slots.

```bash
# TONCore: configure slot 0 (even) — 50% validator share via basis points or percent
nodectl config pool add core --name pool0 --validator-share 5000 --even
nodectl config pool add core --name pool0 --validator-share-percent 50 --even

# Configure both slots with explicit separate commands
nodectl config pool add core \
  --name pool0 \
  --validator-share-percent 50 \
  --min-validator-stake 10000 \
  --even
nodectl config pool add core \
  --name pool0 \
  --validator-share-percent 50.4 \
  --min-validator-stake 10001 \
  --odd

# Configure existing deployed addresses per slot
nodectl config pool add core \
  --name pool0 \
  --address "-1:..." \
  --even
nodectl config pool add core \
  --name pool0 \
  --address "-1:..." \
  --odd
```

##### `config pool ls`

List all configured pools. For **TONCore** slots, if the pool contract is not on-chain yet (or RPC cannot read it), the table shows **not deployed** (or **error** only for real failures) and fills **Share**, **Validator**, **Min nom. stake**, etc. from the **local slot config** when those values were set at `config pool add core` time.

The TONCore table includes a **Src** column showing where deploy-style fields (validator share, stake thresholds) ultimately came from:

| Value | Meaning |
|-------|---------|
| `chain` | Read live from the pool contract via `get_pool_data` |
| `config` | Filled from the local slot config because `get_pool_data` was not called or failed (account not active or RPC error) |
| `-` | Neither chain nor config provided deploy-style fields |

JSON output exposes the same signal as a `data_source` field on each slot (omitted when no deploy-style fields are present).

```bash
nodectl config pool ls
# or with json format
nodectl config pool ls --format json
```

##### `config pool rm`

Remove a pool from the configuration.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Pool name to remove |

```bash
nodectl config pool rm --name pool0
```

##### `config pool deposit-validator` (TONCore only)

Deposit validator funds into a TONCore nominator pool. Builds and sends the on-chain deposit message through the binding's configured wallet.

The pool contract charges a **fixed 1 TON processing fee** on each validator deposit (the credited `validator_amount` is the message value minus that fee). nodectl sends **`--amount` + 1 TON** on-chain so the stake credited to the validator matches **`--amount`**.

| Flag | Short | Description |
|------|-------|-------------|
| `--binding <NAME>` | `-b` | Binding name (resolves wallet and pool) |
| `--amount <TON>` | `-a` | Validator stake to credit (TON); the outbound message value adds the pool’s 1 TON fee |
| `--pool-even` | | Use the pool for even validation rounds (default if neither flag is set) |
| `--pool-odd` | | Use the pool for odd validation rounds |
| `--yes` | | Skip the interactive confirmation prompt |

```bash
nodectl config pool deposit-validator --binding node0 --amount 10000 --pool-even
```

##### `config pool withdraw-validator` (TONCore only)

Withdraw validator funds from a TONCore nominator pool. Same flags as `deposit-validator`.

```bash
nodectl config pool withdraw-validator --binding node0 --amount 5000 --pool-odd
```

---

#### `config bind`

Manage bindings that link nodes to wallets and pools. A binding associates a node with a wallet (required) and optionally a pool for elections participation.

##### `config bind add`

Create a binding between a node, a wallet, and an optional pool. The node and wallet must already exist in the configuration.

| Flag | Short form | Description |
|------|------------|-------------|
| `--node <NAME>` | `-n` | Node name (must exist in `nodes`) |
| `--wallet <NAME>` | `-w` | Wallet name (must exist in `wallets`) |
| `--pool <NAME>` | `-p` | Pool name (optional, must exist in `pools`) |

```bash
# Bind a wallet to a node
nodectl config bind add \
  --node node0 \
  --wallet wallet0

# Bind a wallet and pool to a node
nodectl config bind add \
  --node node0 \
  --wallet wallet0 \
  --pool pool0
```

##### `config bind ls`

List all node bindings.

```bash
nodectl config bind ls
# or with json format
nodectl config bind ls --format json
```

##### `config bind rm`

Remove a node binding.

| Flag | Short form | Description |
|------|------------|-------------|
| `--node <NAME>` | `-n` | Node name to unbind |

```bash
nodectl config bind rm --node node0
```

---

#### `config ton-http-api`

Configure the TON HTTP API connection settings.

##### `config ton-http-api set`

Replace the TON HTTP API endpoint list with a single URL and optional API key.

| Flag | Short form | Description |
|------|------------|-------------|
| `--endpoint <URL>` | `-e` | TON HTTP API endpoint URL |
| `--api-key <KEY>` | `-k` | API key (optional) |

```bash
# Set a single TON HTTP API endpoint
nodectl config ton-http-api set --endpoint "https://toncenter.com/api/v2/jsonRpc"

# Set endpoint with API key
nodectl config ton-http-api set \
  --endpoint "https://toncenter.com/api/v2/jsonRpc" \
  --api-key "your-api-key"
```

##### `config ton-http-api add`

Append one or more failover endpoints (existing endpoints are preserved; duplicates are skipped).

| Flag | Short form | Description |
|------|------------|-------------|
| `--endpoint <URL>` | `-e` | TON HTTP API endpoint URL (repeat to add more than one) |
| `--api-key <KEY>` | `-k` | Per-endpoint API key applied to every URL in this invocation (optional; falls back to the global key) |

```bash
# Add a single failover endpoint
nodectl config ton-http-api add --endpoint "https://backup.example/api/v2/jsonRpc"

# Add multiple failover endpoints in one invocation
nodectl config ton-http-api add \
  --endpoint "https://backup1.example/api/v2/jsonRpc" \
  --endpoint "https://backup2.example/api/v2/jsonRpc"
```

---

#### `config master-wallet`

Manage the master wallet configuration.

##### `config master-wallet info`

Display information about the configured master wallet (address, version, workchain).

```bash
nodectl config master-wallet info
# or with json format
nodectl config master-wallet info --format json
```

---

#### `config log`

Manage log configuration settings (level, output mode, rotation, file path).

##### `config log ls`

Display the current log settings.

| Flag | Short form | Description |
|------|------------|-------------|
| `--format <FORMAT>` | | Output format: `table` or `json` (default: `table`) |

```bash
nodectl config log ls
nodectl config log ls --format json
```

##### `config log set`

Update one or more log settings. Changes are persisted to the config file and will take effect the next time the service is started or restarted.

| Flag | Description |
|------|-------------|
| `--level <LEVEL>` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `--path <PATH>` | Log file path |
| `--rotation <ROTATION>` | Rotation policy: `daily`, `hourly`, `never` |
| `--output <OUTPUT>` | Output mode: `console`, `file`, `all` |
| `--max-size-mb <SIZE>` | Max log file size in MB before rotation |
| `--max-files <COUNT>` | Max number of rotated log files to keep |

```bash
# Set log level to debug
nodectl config log set --level debug

# Configure file logging with rotation
nodectl config log set --output file --path /var/log/nodectl/node.log --rotation daily

# Update multiple settings at once
nodectl config log set --level warn --max-size-mb 100 --max-files 5
```

---

#### `config elections`

Manage elections configuration, including stake policies, tick intervals, AdaptiveSplit50 wait-window fractions (`config elections wait`, alias `wait-pct`), and per-binding election participation.

##### `config elections show`

Display the current elections configuration.

| Flag | Short form | Description |
|------|------------|-------------|
| `--format <FORMAT>` | | Output format: `table` or `json` (default: `table`) |

```bash
nodectl config elections show
nodectl config elections show --format json
```

##### `config elections stake-policy`

Set the default or per-node stake policy in the elections configuration.

| Flag | Short form | Description |
|------|------------|-------------|
| `--fixed <AMOUNT>` | | Fixed stake amount in TON |
| `--split50` | | Use 50% of available balance |
| `--minimum` | | Use minimum required stake |
| `--adaptive-split50` | | Adaptive: split half when above Elector's minimum effective stake, otherwise stake all (see [Staking Strategies](./docs/staking-strategies.md)) |
| `--node <NAME>` | `-n` | Apply policy only to this node (override). Omit to set the default policy. |
| `--reset` | | Remove a per-node policy override (requires `--node`) |

```bash
# Set default policy to minimum stake
nodectl config elections stake-policy --minimum

# Set fixed stake (1000 TON)
nodectl config elections stake-policy --fixed 1000

# Set adaptive split50
nodectl config elections stake-policy --adaptive-split50

# Override policy for a specific node
nodectl config elections stake-policy --node node0 --fixed 500

# Remove a per-node override
nodectl config elections stake-policy --node node0 --reset
```

##### `config elections tick-interval`

Set the elections check interval.

| Argument | Description |
|----------|-------------|
| `<SECONDS>` | Tick interval in seconds |

```bash
nodectl config elections tick-interval 60
```

##### `config elections max-factor`

Set the maximum stake factor for elections. The value must be between **1.0** and the network’s **maximum stake factor** from masterchain **config param 17** (`max_stake_factor`). nodectl does not use a hardcoded upper bound (e.g. 3.0): the CLI reads the current limit from the chain when validating and saving.

| Argument | Description |
|----------|-------------|
| `<VALUE>` | Max factor value |

```bash
nodectl config elections max-factor 2.5
```

##### `config elections wait`

Set the **AdaptiveSplit50 staking window**: earliest stake submission and latest deadline for waiting on peers (fractions of the election duration, **[0.0, 1.0]**). These map to `sleep_period_pct` (`--min`) and `waiting_period_pct` (`--max`) in config.

Subcommand alias: `wait-pct` (same flags).

They apply **only** when the stake policy is **adaptive_split50**; for `minimum`, `split50`, and `fixed` the values are stored but unused.

The service merges with current settings and validates the pair (**min ≤ max**) on **each** update — partial updates are fine (`--min` only, `--max` only, or both).

| Flag | Config field | Description |
|------|----------------|-------------|
| `--min <FRAC>` | `sleep_period_pct` | Earliest fraction at which staking may proceed |
| `--max <FRAC>` | `waiting_period_pct` | Latest fraction for waiting on peers |

At least one flag is required. Run `nodectl config elections wait --help` for full semantics.

```bash
nodectl config elections wait --min 0.15 --max 0.45
nodectl config elections wait --min 0.15
nodectl config elections wait --max 0.45
# equivalent:
nodectl config elections wait-pct --min 0.15 --max 0.45
```

##### `config elections enable`

Enable elections participation for one or more bindings.

| Argument | Description |
|----------|-------------|
| `<NODES>...` | Binding name(s) to enable |

```bash
nodectl config elections enable node0 node1
```

##### `config elections disable`

Disable elections participation for one or more bindings.

| Argument | Description |
|----------|-------------|
| `<NODES>...` | Binding name(s) to disable |

```bash
nodectl config elections disable node0
```

##### `config elections static-adnl`

Generate a persistent ADNL address for a node and save it to the `elections.static_adnls` config. The ADNL key is created on the validator node via its control server. Once set, the election runner reuses this address every cycle instead of generating a fresh one.

| Flag | Short form | Description |
|------|------------|-------------|
| `--node <NAME>` | `-n` | Node name (must exist in `nodes`) |

```bash
nodectl config elections static-adnl --node node0
```

---

### Key Management Commands

Commands for managing keys in the secret vault. To connect secret vault to the cli define an environment variable `VAULT_URL`.

#### `key add`

Generate a new cryptographic key and store it in the vault.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Key name (unique identifier in the vault) |
| `--algorithm <ALG>` | `-a` | Algorithm (default: `ed25519`) |
| `--extractable` | `-e` | Mark key as extractable (allows exporting the private key) |

```bash
# Generate a new key (e.g. for a wallet — non-extractable)
nodectl key add --name "wallet0-key"

# Generate an extractable key (e.g. for a control client / ADNL connection)
nodectl key add --name "control-client-key" --extractable
```

---

#### `key import`

Import an existing private key into the vault.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Key name (unique identifier in the vault) |
| `--private-key <KEY>` | `-k` | Private key (base64) |
| `--algorithm <ALG>` | `-a` | Algorithm (default: `ed25519`) |
| `--extractable` | `-e` | Mark key as extractable |

```bash
nodectl key import \
  --name "wallet0-key" \
  --private-key "base64-encoded-private-key"
```

---

#### `key ls`

List all keys stored in the vault.

```bash
nodectl key ls
```

---

#### `key rm`

Remove a key from the vault.

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Key name to remove |

```bash
nodectl key rm --name "old-key"
```

---

### Authentication Commands

Commands for managing REST API users and tokens. User credentials are stored in the vault. For a detailed description of roles, token lifecycle, revocation, rate limiting, and monitoring, see the **[Security Guide](./docs/nodectl-security.md)**.

#### `auth add`

Create a new API user. The password is entered interactively and confirmed.

| Flag | Description |
|------|-------------|
| `--username <NAME>` | Username (alphanumeric, `_`, `-`, max 64 chars) |
| `--role <ROLE>` | User role: `operator` or `nominator` |

```bash
# Create an operator user (full operational access)
nodectl auth add --username admin --role operator

# Create a nominator user (read-only status access)
nodectl auth add --username viewer --role nominator
```

---

#### `auth ls`

List all configured users.

```bash
nodectl auth ls
```

---

#### `auth rm`

Remove a user.

| Argument | Description |
|----------|-------------|
| `<USERNAME>` | Username to remove |

```bash
nodectl auth rm admin
```

---

#### `auth revoke`

Revoke all tokens issued to a user. After revocation the user can log in again to obtain a new token.

| Argument / Flag | Description |
|-----------------|-------------|
| `<USERNAME>` | Username whose tokens to revoke |
| `--at <TIMESTAMP>` | Optional unix timestamp cutoff (default: now) |

```bash
# Revoke all current tokens
nodectl auth revoke admin

# Revoke tokens issued before a specific time
nodectl auth revoke admin --at 1710000000
```

---

#### `auth set ttl`

Configure token TTL (time-to-live) for each role.

| Flag | Description |
|------|-------------|
| `--operator <DURATION>` | Operator token TTL (e.g. `3600`, `30s`, `60m`, `8h`) |
| `--nominator <DURATION>` | Nominator token TTL |

```bash
nodectl auth set ttl --operator 8h --nominator 1h
```

---

### Deploy Commands

Commands for deploying contracts to the blockchain. Requires a configuration file with `ton_http_api` and `wallets` sections.

Note: wallet will be deployed only if the wallet account has at least 0.1 TON.

#### `deploy wallet`

Deploy validator wallets defined in the configuration.

| Flag | Short form | Description |
|------|------------|-------------|
| `--config <FILE>` | `-c` | Path to the configuration file. Can also be set as an environment variable CONFIG_PATH |
| `--node <NAME>` | | Deploy a specific wallet by wallet name (as defined in `config wallet add --name`; mutually exclusive with `--all`) |
| `--all` | | Deploy all wallets (mutually exclusive with `--node`) |
| `--verbose` | | Print deployment progress |


```bash
# Deploy a specific wallet by wallet name
nodectl deploy wallet --config config.json --node wallet0

# Deploy all wallets with verbose output
nodectl deploy wallet --config config.json --all --verbose
```

---

#### `deploy pool`

Deploy a nominator pool contract. The pool type comes from the binding’s `pool` entry in the config (Single Nominator Pool, single-pool TONCore, or TONCore nominator with two pools).

| Option | Short form | Description |
|--------|------------|-------------|
| `--config <FILE>` | `-c` | Path to the configuration file. Can also be set as an environment variable CONFIG_PATH |
| `--binding <NAME>` | `-b` | Binding name (resolves the validator wallet and pool from config) |
| `--amount <TON>` | | Amount of TON to transfer for deployment |
| `--owner <ADDRESS>` | | **SNP only:** pool owner address |
| `--pool-even` | | **TONCore nominator only:** deploy the pool for even validation rounds (default if neither flag is set) |
| `--pool-odd` | | **TONCore nominator only:** deploy the pool for odd validation rounds |
| `--verbose` | | Print deployment progress |

```bash
# Single Nominator Pool (requires --owner)
nodectl deploy pool \
  --config config.json \
  --binding my-binding \
  --owner "-1:owner_address_here" \
  --amount 1.5

# TONCore nominator: deploy the second pool (odd rounds)
nodectl deploy pool \
  --config config.json \
  --binding my-binding \
  --amount 1.5 \
  --pool-odd
```

The command derives the pool address from the wallet and config, sends a deploy message with the specified amount, and waits for the contract to become active. The result is printed as JSON with the pool address and deployment status.

> **Note**: The validator wallet must be in the `Active` state and have enough balance to cover the transfer amount. If the pool is already deployed, the command exits without sending a transaction.

---

### Service Command

#### `service`

Start nodectl as a background service (daemon mode). Requires a configuration file.

| Flag | Short form | Description |
|------|------------|-------------|
| `--config <FILE>` | `-c` | Path to the configuration file. Can also be set as an environment variable CONFIG_PATH |

```bash
# Start service
nodectl service -c config.json

# With debug logging (via env)
RUST_LOG=debug nodectl service -c config.json
```

---

### Service API Commands

Commands for interacting with the nodectl service REST API. The service URL is resolved in this order: explicit `--url`, then `http.bind` from `--config`. If neither is available, the command fails. When connecting from a remote machine, pass `--url` explicitly.

#### `api`

Client for the nodectl service REST API. Use this to interact with a running nodectl daemon.

| Flag | Short form | Description |
|------|------------|-------------|
| `--config <FILE>` | `-c` | Path to configuration file (reads `http.bind` for the service URL; default: `nodectl-config.json`). Can also be set via `CONFIG_PATH` env var |
| `--url <URL>` | `-u` | URL to the node control service API. Takes precedence over `--config` when both are provided |
| `--token <TOKEN>` | | JWT token for authentication (env: `NODECTL_API_TOKEN`) |

**Subcommands:**

##### `api login`

Authenticate with the REST API and obtain a JWT token. The password is entered interactively unless `--password-stdin` is used.

| Argument / Flag | Description |
|-----------------|-------------|
| `<USERNAME>` | Username to authenticate with |
| `--password-stdin` | Read password from stdin (for non-interactive use) |

```bash
# Interactive login
nodectl api login admin

# Non-interactive (e.g. in scripts)
echo "$PASSWORD" | nodectl api login admin --password-stdin
```

The command returns the JWT token, its expiration time, and the user role. Store the token for subsequent API calls:

```bash
export NODECTL_API_TOKEN="<token from login>"
```

Once the token is exported, all `nodectl api` commands use it automatically.

##### `api health`

Check service health.

```bash
nodectl api health
nodectl api --url http://localhost:8080 health
```

##### `api elections`

Get current elections snapshot. Optionally exclude or include nodes from elections participation.

| Flag | Description |
|------|-------------|
| `--exclude <NODES>` | Comma-separated list of nodes to exclude from elections |
| `--include <NODES>` | Comma-separated list of nodes to include in elections |

```bash
# Get elections status
nodectl api elections

# Exclude nodes from elections
nodectl api elections --exclude node0,node1

# Include nodes back into elections
nodectl api elections --include node0,node1
```

##### `api validators`

Get validators snapshot (only for controlled nodes from configuration file):

```bash
nodectl api validators
```

##### `api task`

Control background tasks (elections, voting).

| Argument | Description |
|----------|-------------|
| `<name>` | Task name: `elections` or `voting` |
| `<action>` | Action: `enable`, `disable`, or `restart` |

```bash
# Disable elections task
nodectl api task elections disable

# Enable elections task
nodectl api task elections enable

# Restart elections task
nodectl api task elections restart
```

##### `api stake-policy`

Set the stake policy for elections on a running service. Use `--node` to set a per-node override instead of changing the default policy. This command is a shortcut for `POST /v1/elections/settings`.

| Flag | Short form | Description |
|------|------------|-------------|
| `--fixed <AMOUNT>` | | Fixed stake amount (in nanoTON) |
| `--split50` | | Use 50% of available balance |
| `--minimum` | | Use minimum required stake |
| `--adaptive-split50` | | Adaptive: split half when above Elector's minimum effective stake, otherwise stake all |
| `--node <NAME>` | `-n` | Apply policy only to this node (per-node override). Omit to set the default policy. |

```bash
# Set minimum stake policy (default for all nodes)
nodectl api stake-policy --minimum

# Set fixed stake (1000 TON = 1000000000000 nanoTON)
nodectl api stake-policy --fixed 1000000000000

# Set split50 policy
nodectl api stake-policy --split50

# Set adaptive split50
nodectl api stake-policy --adaptive-split50

# Override policy for a specific node
nodectl api stake-policy --node node0 --fixed 500000000000
```

---

### TON HTTP API

Commands for retrieving data from the blockchain via TON HTTP API:

#### `config-param`

Get a configuration parameter from the blockchain (via TON HTTP API).

| Flag / Argument | Short form | Description |
|-----------------|------------|-------------|
| `--config <FILE>` | `-c` | Path to configuration file (provides `ton_http_api` settings). Can also be set as an environment variable CONFIG_PATH |
| `<ID>` | | Configuration parameter ID |

```bash
nodectl config-param -c config.json 34
```

---

### Voting Commands

`nodectl vote` is a **REST client** to the running nodectl service. The daemon calls the **Config contract** on-chain (via the service’s configured TON HTTP API) and persists `voting.proposals` with **`update_and_save`**, like other centralised config flows.

- **Service URL:** `--url` / `NODECTL_URL`, or `http.bind` from `--config` (default `nodectl-config.json`), or `http://127.0.0.1:8080`.
- **JWT:** `--token` / `NODECTL_API_TOKEN`. **`vote ls`**, **`inspect`**, and read-only config used for interactive flows require a **nominator** (or **operator**) token. **`vote add`** and **`vote rm`** require an **operator** token.

| Flag | Short | Description |
|------|-------|-------------|
| `--url <URL>` | `-u` | Service base URL (overrides config; env: `NODECTL_URL`) |
| `--token <TOKEN>` | | Bearer JWT (env: `NODECTL_API_TOKEN`) |
| `--config <FILE>` | `-c` | Config path for `http.bind` when `--url` is not set (env: `CONFIG_PATH`) |

#### `vote ls`

List active config proposals (from the network). Proposals already tracked in `voting.proposals` are marked with `*`.

| Flag | Description |
|------|-------------|
| `--format <FORMAT>` | `table` (default) or `json` |

```bash
export NODECTL_API_TOKEN="..."   # nominator or operator
nodectl vote ls
nodectl vote ls --url http://127.0.0.1:8080 --token "$NODECTL_API_TOKEN"
```

#### `vote inspect`

Details for a proposal: `expires_in`, voters, weight remaining, param cell BOC (base64), param hash. Hash: **64 hex characters** (32 bytes).

```bash
nodectl vote inspect <HEX64>
nodectl vote inspect <HEX64> --format json
```

#### `vote add`

Track a proposal (must exist among **active** on-chain proposals). Use `--hash` or run interactively (select from the list). Idempotent: if the hash is already tracked, the command succeeds without a second write.

```bash
nodectl vote add --hash <HEX64>   # requires operator token
nodectl vote add                 # interactive; operator token
```

#### `vote rm`

Remove a hash from the tracked list. Use `--hash` or interactive selection from the current `voting.proposals` list.

```bash
nodectl vote rm --hash <HEX64>
nodectl vote rm
```

---

## REST API Endpoints

When running in service mode, nodectl exposes a REST API for monitoring and management. By default, the HTTP server listens on all interfaces (`0.0.0.0:8080`) with authentication enabled and no users — all protected endpoints return `401` until at least one user is created via `nodectl auth add`. Protected endpoints require a JWT token in the `Authorization: Bearer <token>` header. See the **[Security Guide](./docs/nodectl-security.md)** for full details on roles, rate limiting, and token revocation.

> **Warning:** nodectl serves plain HTTP. If the API is reachable outside your trusted network, terminate TLS at a reverse proxy or load balancer — otherwise passwords (`/auth/login`) and JWT tokens (`Authorization` header) travel in plain text.

### Configuration

The HTTP server is configured in the `http` section of the config:

```json
{
  "http": {
    "bind": "0.0.0.0:8080",
    "enable_swagger": true
  }
}
```

### OpenAPI / Swagger

- **OpenAPI spec**: `GET /openapi.json`
- **Swagger UI**: `GET /swagger` or `GET /swagger-ui` (when `enable_swagger: true`)

### Endpoints

All endpoints return `{ "ok": true, ... }` on success and `{ "ok": false, "error": { "code": <http>, "message": "..." } }` on failure. `200` indicates success; `400`, `401`, `403`, `404`, `429`, `500` are used as documented.

Role columns use the following shorthand: **P** = public (no token), **N** = `nominator` or `operator`, **O** = `operator` only.

| Method | Path | Role | Summary |
|--------|------|------|---------|
| GET | `/health` | P | Health check |
| GET | `/openapi.json` | P | OpenAPI spec |
| GET | `/swagger`, `/swagger-ui` | P | Swagger UI (when `enable_swagger` is `true`) |
| POST | `/auth/login` | P | Exchange username/password for a JWT |
| GET | `/auth/me` | N | Identity of the authenticated user |
| GET | `/auth/users` | O | List users |
| GET | `/v1/elections` | N | Current elections snapshot |
| POST | `/v1/elections/exclude` | O | Disable elections for given bindings |
| POST | `/v1/elections/include` | O | Enable elections for given bindings |
| GET | `/v1/elections/settings` | N | Elections configuration (policy, overrides, tick, max-factor, adaptive sleep/wait fractions, per-binding status) |
| POST | `/v1/elections/settings` | O | Update elections settings (policy, per-node override, tick, max-factor, `sleep_period_pct`, `waiting_period_pct`) |
| GET | `/v1/automation/settings` | N | Contracts task settings (auto-deploy, auto-topup, amounts, tick) |
| POST | `/v1/automation/settings` | O | Update contracts task settings (partial JSON: tick/toggles and nested `wallet` / `pool`, nanotons) |
| POST | `/v1/elections/static-adnl` | O | Generate and assign a persistent ADNL address for a node |
| GET | `/v1/validators` | N | Validators snapshot for controlled nodes |
| POST | `/v1/task/elections` | O | Enable / disable / restart the elections background task |
| GET | `/v1/nodes` | N | List configured nodes with control-server status |
| POST | `/v1/nodes` | O | Add a node |
| DELETE | `/v1/nodes/{name}` | O | Remove a node |
| GET | `/v1/wallets` | N | List configured wallets with on-chain state |
| POST | `/v1/wallets` | O | Add a wallet |
| DELETE | `/v1/wallets/{name}` | O | Remove a wallet |
| GET | `/v1/pools` | N | List configured pools with live balances |
| POST | `/v1/pools` | O | Add a pool (SNP or TONCore) |
| DELETE | `/v1/pools/{name}` | O | Remove a pool |
| GET | `/v1/bindings` | N | List node bindings |
| POST | `/v1/bindings` | O | Add a binding |
| DELETE | `/v1/bindings/{node}` | O | Remove a binding (requires `idle` status) |
| GET | `/v1/voting/config` | N | Voting section snapshot (`proposals`, `tick_interval`) |
| GET | `/v1/voting/proposals` | N | Active on-chain proposals with `tracked` flag |
| GET | `/v1/voting/proposals/{hash}` | N | Single proposal details (Config contract) |
| POST | `/v1/voting/proposals` | O | Add proposal hash to tracked list |
| DELETE | `/v1/voting/proposals/{hash}` | O | Remove proposal hash from tracked list |
| GET | `/v1/log` | N | Current log configuration |
| POST | `/v1/log` | O | Update log settings |
| POST | `/v1/ton-http-api` | O | Replace or append TON HTTP API endpoints |
| GET | `/v1/master-wallet` | N | Master wallet address, balance, version |

#### `GET /health`

```json
{ "ok": true, "result": "OK" }
```

---

#### `POST /auth/login`

Authenticate and obtain a JWT token. Rate-limited: 5 failed attempts per 60s window, then blocked for 120s.

**Request:**

```json
{ "username": "admin", "password": "secret" }
```

**Response:**

```json
{
  "ok": true,
  "token": "<JWT>",
  "expires_in": 2592000,
  "role": "operator"
}
```

---

#### `GET /auth/me`

Return the identity of the authenticated user.

```json
{ "ok": true, "username": "admin", "role": "operator" }
```

---

#### `GET /auth/users`

List all users.

```json
{
  "ok": true,
  "users": [
    { "username": "admin", "role": "operator" },
    { "username": "viewer", "role": "nominator" }
  ]
}
```

---

#### `GET /v1/elections`

Current elections snapshot. Pass `?include_participants=true` to include the full participants list (omitted by default to keep the response small).

**Response:**

```json
{
  "ok": true,
  "status": "active",
  "result": {
    "election_id": 1734523200,
    "elect_close": 1734522300,
    "elect_close_utc": "2024-12-18 10:45:00",
    "finished": false,
    "failed": false,
    "participants_count": 42,
    "min_stake": "300000000000000",
    "participant_min_stake": "400000000000000",
    "participant_max_stake": "1200000000000000",
    "participants": []
  },
  "next_elections": {
    "start": 1734530000,
    "start_utc": "2024-12-18 12:00:00",
    "end": 1734616400,
    "end_utc": "2024-12-19 12:00:00"
  },
  "our_participants": []
}
```

- `status` is one of `closed`, `active`, `finished`, `failed`, `postponed`.
- `result` is `null` when no snapshot has been collected yet.
- Stake fields (`min_stake`, `participant_*`, per-participant `stake`) are **strings** of nanotons (decimal).
- `our_participants[]` lists each controlled node's participation lifecycle (`status`: `idle → participating → submitted → accepted → elected → validating`) and stake submission history.

---

#### `POST /v1/elections/exclude`

Disable elections for the given bindings (sets `enable: false`). Triggers an elections task restart.

**Request:**

```json
{ "nodes": ["node0", "node1"] }
```

**Response:**

```json
{
  "ok": true,
  "result": {
    "excluded": ["node0", "node1"],
    "updated_at": 1734523200
  }
}
```

---

#### `POST /v1/elections/include`

Enable elections for the given bindings (sets `enable: true`). Same response shape as `/exclude` — `excluded` lists bindings that remain disabled.

```json
{ "nodes": ["node0"] }
```

---

#### `GET /v1/elections/settings`

Return the effective elections configuration plus per-binding status.

**Response:**

```json
{
  "ok": true,
  "result": {
    "stake_policy": "split50",
    "policy_overrides": { "node0": { "fixed": 500000000000 } },
    "max_factor": 3.0,
    "tick_interval": 40,
    "sleep_period_pct": 0.2,
    "waiting_period_pct": 0.4,
    "bindings": [
      {
        "name": "node0",
        "enable": true,
        "status": "validating",
        "stake_policy": { "fixed": 500000000000 }
      }
    ]
  }
}
```

---

#### `POST /v1/elections/settings`

Unified endpoint for updating elections settings. Replaces the pre-0.4 `POST /v1/stake_strategy`, `POST /v1/elections/tick-interval`, and `POST /v1/elections/max-factor`. At least one field must be set.

| Field | Type | Description |
|-------|------|-------------|
| `policy` | `StakePolicy` | Stake policy to set; ignored when `reset` is `true` |
| `node` | `string` | Target node for a per-node override (omit for the default policy) |
| `reset` | `bool` | Remove a per-node override (requires `node`) |
| `tick_interval` | `u64` | Elections tick interval in seconds |
| `max_factor` | `f32` | Validated against masterchain config param 17 |
| `sleep_period_pct` | `f64` | AdaptiveSplit50 minimum wait as a fraction of election duration; must be in `[0.0, 1.0]` and ≤ `waiting_period_pct` |
| `waiting_period_pct` | `f64` | AdaptiveSplit50 maximum wait for participants; must be in `[0.0, 1.0]` and ≥ `sleep_period_pct` |

`StakePolicy` is `"minimum"`, `"split50"`, `"adaptive_split50"`, or `{ "fixed": <nanotons> }`.

**Example — update policy + tick interval + max factor in one call:**

```json
{
  "policy": "adaptive_split50",
  "tick_interval": 60,
  "max_factor": 2.5
}
```

**Per-node override:**

```json
{ "policy": { "fixed": 500000000000 }, "node": "node0" }
```

**Reset a per-node override:**

```json
{ "reset": true, "node": "node0" }
```

**Response** — returns the new settings (same shape as `GET` but without `bindings`):

```json
{
  "ok": true,
  "result": {
    "stake_policy": "adaptive_split50",
    "policy_overrides": {},
    "max_factor": 2.5,
    "tick_interval": 60,
    "sleep_period_pct": 0.2,
    "waiting_period_pct": 0.4,
    "bindings": []
  }
}
```

---

#### `GET /v1/automation/settings`

Returns the **`automation`** settings as JSON: `tick_interval_sec`, `auto_deploy`, `auto_topup`, nested **`wallet`** (`deploy`, `topup`, `threshold`) and **`pool`** (`snp`, `ton_core`). All monetary fields are in **nanotons**. See [Contracts automation](./docs/contracts-automation.md).

#### `POST /v1/automation/settings`

**Operator only.** Partial update: include only keys to change (same shape as `GET` `result`, including nested `wallet` / `pool` objects with any subset of their fields). At least one field required. Invalid combinations are rejected with `400` (e.g. tick out of range, zero amounts). Example:

```json
{ "tick_interval_sec": 60, "auto_topup": false, "pool": { "ton_core": 2000000000 } }
```

---

#### `POST /v1/elections/static-adnl`

Generate a persistent ADNL address on the validator node and save it to the `elections.static_adnls` config map. The election runner will reuse this address every cycle instead of generating a fresh ephemeral one.

**Request:**

```json
{ "node": "node0" }
```

**Response:**

```json
{
  "ok": true,
  "result": {
    "adnl_addr": "<base64>"
  }
}
```

Calling this endpoint again for the same node generates a **new** key and overwrites the previous one.

---

#### `GET /v1/validators`

Validators snapshot for controlled nodes only.

**Response:**

```json
{
  "ok": true,
  "result": {
    "controlled_nodes": [
      {
        "node_id": "node0",
        "is_validator": true,
        "validator_index": 42,
        "weight": 1000,
        "wallet_addr": "-1:...",
        "stake": "25000000000000",
        "stake_accepted": true,
        "key_election_id": 1734400000,
        "key_expires_at_utc": "2024-12-19 08:00:00",
        "is_key_active": true,
        "key_id": "<base64>",
        "pubkey": "<base64>",
        "adnl": "<base64>",
        "binding_status": "validating"
      }
    ],
    "default_stake_policy": "split50",
    "validation_range": {
      "start": 1734400000,
      "start_utc": "2024-12-17 08:00:00",
      "end": 1734486400,
      "end_utc": "2024-12-18 08:00:00"
    }
  }
}
```

---

#### `POST /v1/task/elections`

Control the elections background task.

**Request:**

```json
{ "action": "enable" }
```

`action` is `enable`, `disable`, or `restart`.

**Response:**

```json
{
  "ok": true,
  "result": {
    "enabled": true,
    "status": "running",
    "updated_at": 1734523200
  }
}
```

`status` is `running` or `stopped`.

---

#### `GET /v1/nodes`

List configured nodes with a concurrent ADNL connectivity probe (5s timeout).

```json
{
  "ok": true,
  "result": [
    {
      "name": "node0",
      "control_server_endpoint": "192.168.1.100:50000",
      "control_server_pubkey": "<base64>",
      "control_client_secret": "node0-adnl-key",
      "status": "ok"
    }
  ]
}
```

`status` is `ok`, `unknown`, or an error message (e.g. `timeout`, connection failure).

#### `POST /v1/nodes`

Add a node.

**Request:**

```json
{
  "name": "node0",
  "control_server_endpoint": "192.168.1.100:50000",
  "control_server_pubkey": "<base64>",
  "control_client_secret": "node0-adnl-key"
}
```

**Response:**

```json
{ "ok": true, "result": { "name": "node0" } }
```

#### `DELETE /v1/nodes/{name}`

Remove a node. `400` if a binding still references it.

```json
{ "ok": true, "result": { "name": "node0" } }
```

---

#### `GET /v1/wallets`

List configured wallets (including the `master_wallet` slot when present) with live on-chain state. Addresses are bounceable URL-safe base64.

```json
{
  "ok": true,
  "result": [
    {
      "name": "wallet0",
      "secret": "wallet0-key",
      "version": "V3R2",
      "state": "active",
      "balance": 1200000000000,
      "address": "EQ..."
    }
  ]
}
```

`state` / `balance` are `null` when the TON HTTP API is unreachable.

#### `POST /v1/wallets`

```json
{
  "name": "wallet0",
  "secret": "wallet0-key",
  "version": "V3R2",
  "subwallet_id": 42,
  "workchain": -1
}
```

`400` if the name is `master_wallet` (reserved) or already exists.

#### `DELETE /v1/wallets/{name}`

Remove a wallet. Refuses to delete `master_wallet`; `400` if referenced by a binding.

---

#### `GET /v1/pools`

List configured pools with live balances and, for TONCore, per-slot addresses and validator share.

```json
{
  "ok": true,
  "result": [
    {
      "name": "pool0",
      "kind": "SNP",
      "balance": 900000000000,
      "address": "EQ...",
      "owner": "EQ..."
    },
    {
      "name": "pool1",
      "kind": "Core",
      "balance": null,
      "address": null,
      "owner": null,
      "addresses": ["EQ...", "<not deployed>"],
      "validator_share": 5000
    }
  ]
}
```

#### `POST /v1/pools`

Add a pool (SNP or TONCore). At least one of `address` / `owner` is required for SNP; TONCore requires `kind: "core"` plus a `slot` (`even` / `odd`) and either `address` or `params` (`validator_share`, `max_nominators`, `min_validator_stake`, `min_nominator_stake`).

**SNP example:**

```json
{ "name": "pool0", "owner": "EQ..." }
```

#### `DELETE /v1/pools/{name}`

`400` if a binding still references the pool.

---

#### `GET /v1/bindings`

```json
{
  "ok": true,
  "result": [
    { "node": "node0", "wallet": "wallet0", "pool": "pool0", "enable": true, "status": "validating" }
  ]
}
```

#### `POST /v1/bindings`

```json
{ "node": "node0", "wallet": "wallet0", "pool": "pool0" }
```

The node and wallet must already exist. A pool may be bound to at most one node.

#### `DELETE /v1/bindings/{node}`

Requires the binding to be in `idle` status. If it is `participating`, `draining`, or `validating`, disable elections first and wait for stake recovery.

---

#### `GET /v1/log`

```json
{
  "ok": true,
  "result": {
    "level": "INFO",
    "path": "./logs/nodectl.log",
    "rotation": "daily",
    "output": "all",
    "max_size_mb": 50,
    "max_files": 10
  }
}
```

#### `POST /v1/log`

At least one field must be set. Setting `output` to `file` / `all` requires an existing `path` (either previously configured or provided in the same request).

```json
{ "level": "debug", "output": "all", "path": "/var/log/nodectl/service.log" }
```

---

#### `POST /v1/ton-http-api`

Replace or append TON HTTP API endpoints.

**Request:**

```json
{
  "urls": ["https://toncenter.com/api/v2/jsonRpc"],
  "api_key": "your-api-key",
  "append": false
}
```

When `append` is `true`, the URLs are added after existing ones (duplicates skipped) and `api_key` applies to the newly added entries; when `false` (default), the list is fully replaced.

**Response:**

```json
{ "ok": true, "result": { "endpoints": ["https://toncenter.com/api/v2/jsonRpc"] } }
```

---

#### `GET /v1/master-wallet`

```json
{
  "ok": true,
  "result": {
    "address": "EQ...",
    "balance": 25000000000,
    "state": "active",
    "version": "V3R2",
    "subwallet_id": 42,
    "secret": "master-wallet-secret",
    "public_key": "<base64>"
  }
}
```

---

## Configuration

Configuration is specified in JSON format.

### Config Structure

```json
{
  "nodes": {
    "<node_name>": {
      "server_address": "<IP/DOMAIN NAME>:<PORT>",
      "server_key": { "type_id": 1209251014, "pub_key": "<BASE64>" },
      "client_key": { "type_id": 1209251014, "pvt_key": "<BASE64>" } | { "name": "<VAULT_SECRET_NAME>" },
      "timeouts": 5
    }
  },
  "wallets": {
    "<node_name>": {
      "key": "<HEX_PRIVATE_KEY>" | { "name": "<VAULT_SECRET_NAME>" },
      "version": "V1R3" | "V3R2" | "V4R2" | "V5R1",
      "subwallet_id": 42,
      "workchain": -1
    }
  },
  "pools": {
    "<pool_name>": {
      "kind": "snp",
      "address": "-1:<POOL_ADDRESS>",
      "owner": "-1:<OWNER_ADDRESS>"
    }
  },
  "bindings": {
    "<node_name>": {
      "wallet": "<wallet_name>",
      "pool": "<pool_name>",
      "enable": true
    }
  },
  "ton_http_api": {
    "urls": [
      "http://127.0.0.1:3301/",
      { "url": "https://backup.example/api/v2/jsonRpc", "api_key": "<PER_ENDPOINT_KEY>" }
    ],
    "api_key": "<OPTIONAL_GLOBAL_KEY>" | null
  },
  "http": {
    "bind": "0.0.0.0:8080",
    "enable_swagger": true,
    "auth": { /* see http.auth below; omit or set to null to disable auth */ }
  },
  // optional
  "master_wallet": {
    "key": { "name": "<VAULT_SECRET_NAME>" },
    "version": "V3R2",
    "subwallet_id": 42,
    "workchain": 0
  },
  // optional
  "elections": {
    "policy": "split50" | "minimum" | "adaptive_split50" | { "fixed": 1000000000000 },
    "policy_overrides": { "<node_name>": "minimum" | { "fixed": <amount> } | "split50" | "adaptive_split50" },
    "max_factor": 3.0,
    "tick_interval": 40,
    "sleep_period_pct": 0.2,
    "waiting_period_pct": 0.4,
    "static_adnls": { "<node_name>": "<base64_adnl_key_hash>" }
  },
  // optional
  "voting": {
    "proposals": [],
    "tick_interval": 40
  },
  // optional
  "log": {
    "path": "logs/service.log",
    "max_size_mb": 50,
    "max_files": 10,
    "rotation": "daily",
    "level": "INFO",
    "output": "console"
  },
  "tick_interval": 40
}
```

### Section Descriptions

#### `nodes`

Connection configuration for nodes via ADNL (Control Server):

- `server_address` — IP address/domain name and port of the node's Control Server
- `server_key` — server public key (inline `{ "type_id": ..., "pub_key": "..." }` or vault reference `{ "name": "..." }`)
- `client_key` — client private key for authentication (inline `{ "type_id": ..., "pvt_key": "..." }` or vault reference `{ "name": "..." }`)
- `timeouts` — connection timeout in seconds (single number) or detailed timeouts `{ "read": {...}, "write": {...} }`

#### `wallets`

Validator wallets for election submissions and TON transfers:

- `key` — wallet private key (hex string, 64 bytes) or vault reference `{ "name": "..." }`
- `version` — wallet version (`V1R3`, `V3R2`, `V4R2`, `V5R1`)
- `subwallet_id` — subwallet ID. Has no effect for `V1R3` wallets, which do not have a subwallet concept
- `workchain` — workchain ID (default: `-1`)

#### `pools`

Nominator pool configurations. Pool `kind` is **`"snp"`** or **`"core"`**.

**Single Nominator Pool (SNP):**

- `kind` — `"snp"`
- `address` — deployed pool contract address (optional)
- `owner` — pool owner address (optional)

**TONCore (`kind: "core"`):**

- `pools` — JSON array of **exactly two** elements: `pools[0]` and `pools[1]`. Each element is either `null` (slot unused) or an object:
  - `address` — optional deployed pool contract address (raw / base64url string). When omitted but `params` is set, the address is derived from the validator and `params` (see `resolve_toncore_pool` / `toncore_pool_address_and_state` in the contracts crate). If `address` is set, it must match the derived address when `params` is present.
  - `params` — optional `TonCoreInitParams`: `validator_share`, `max_nominators`, `min_validator_stake`, `min_nominator_stake` (nanotons on-chain). Omit fields to use defaults from `app_config` / serde.
- A **single** on-chain pool is `pools: [ { ... }, null ]` (or `[null, { ... }]`). **Two** pools require **two** non-null entries with parameters and/or addresses you define — there is no implicit second pool and no automatic `min_validator_stake + 1` between slots.
- **Behaviour:** with two slots, the service uses a **`TonCoreNominatorRouter`**. The election runner picks a **free** pool for staking (`get_pool_data()` / inner pools). Matching finished-election participants uses **both** pool addresses (`inner_pools`).

#### `bindings`

Bindings link nodes to wallets and pools for elections participation:

- `wallet` — wallet name (must reference a key in `wallets`)
- `pool` — pool name (optional, must reference a key in `pools`)
- `enable` — whether this binding participates in elections (default: `false`)
- `status` — current binding status: `idle`, `participating`, `draining`, `validating` (managed automatically)

#### `ton_http_api`

TON HTTP API configuration:

- `urls` — ordered list of JSON-RPC endpoints. Each entry is either a plain URL string (uses the global `api_key`) or an object `{ "url": "...", "api_key": "..." }` with its own key. The first entry is the primary endpoint; the rest are used for failover. Default: `["http://127.0.0.1:3301/"]`.
- `api_key` — global API key used for entries that don't specify their own (optional)

> **Backward compatibility:** the legacy single-URL field `url` is still accepted on load and transparently migrated into the head of `urls` on the next save.

#### `http`

HTTP REST API server configuration:

- `bind` — address and port to bind (default: `0.0.0.0:8080`)
- `enable_swagger` — enable Swagger UI at `/swagger` (default: `true`)
- `auth` — JWT authentication configuration (see below). Omit or set to `null` to disable authentication.

#### `http.auth`

REST API authentication settings. **Authentication is enabled by default** — a freshly generated config includes the `http.auth` section with an empty user list, so all protected endpoints return `401` until at least one user is created via `nodectl auth add`. To disable authentication and open all endpoints, remove the `http.auth` section from the config (or set it to `null`).

> **Note:** On first start the service creates a JWT signing key in the vault (secret `auth.jwt-signing-key`).
>
> **No restart required:** The service hot-reloads the configuration, so changes to users or auth settings take effect immediately.

- `operator_token_ttl` — operator token TTL in seconds (default: `2592000` — 30 days)
- `nominator_token_ttl` — nominator token TTL in seconds (default: `86400` — 1 day)
- `min_password_length` — minimum password length (default: `8`)
- `jwt_secret` — base64-encoded JWT signing key (optional; falls back to vault secret `auth.jwt-signing-key`)
- `users` — list of user entries (managed via `nodectl auth` commands)

#### `master_wallet` (optional)

Master wallet configuration, used for administrative operations. Same structure as wallet entries in the `wallets` section.

#### `elections` (optional)

Automatic elections task configuration:

- `policy` — default stake policy (applies to all nodes unless overridden):
  - `"split50"` — splits all available funds into two equal stakes (default)
  - `"minimum"` — use minimum required stake
  - `"adaptive_split50"` — adaptive: splits half when above the Elector's estimated minimum effective stake for the current round, otherwise stakes all. See [Staking Strategies](./docs/staking-strategies.md).
  - `{ "fixed": <amount> }` — fixed stake amount in nanoTON
- `policy_overrides` — per-node stake policy overrides (node name -> policy). When a node has an entry here, it takes precedence over the default `policy`. Example: `{ "node0": { "fixed": 500000000000 } }`
- `max_factor` — maximum stake factor (default `3.0` in generated configs). Valid values lie in `[1.0, network_max_factor]`, where **`network_max_factor` comes from masterchain config param 17** (`max_stake_factor`); the CLI and stake command validate against the live network when TON HTTP API is available
- `tick_interval` — interval between election checks in seconds (default: `40`)
- `sleep_period_pct` — AdaptiveSplit50 minimum wait as a fraction of election duration. Default `0.2`. Must be in `[0.0, 1.0]` and ≤ `waiting_period_pct`.
- `waiting_period_pct` — AdaptiveSplit50 maximum wait for enough participants as a fraction of election duration. Default `0.4`. Must be in `[0.0, 1.0]` and ≥ `sleep_period_pct`.
- `static_adnls` — pre-generated persistent ADNL addresses keyed by node name (base64-encoded). When a node has an entry here, the runner reuses this ADNL address each election cycle instead of generating a fresh one. Managed via `config elections static-adnl` or `POST /v1/elections/static-adnl`. Example: `{ "node0": "oRvD1E5F..." }`

#### `automation` (optional)

Settings for the **contracts task** (auto-deploy of validator wallets and nominator pools, auto-topup of validator wallets, separate deploy amounts for SNP vs TONCore pools, contracts task tick interval, toggles). Amounts are **nanotons**, grouped under **`wallet`** (`deploy`, `topup`, `threshold`) and **`pool`** (`snp`, `ton_core`). Omitted fields use the built-in defaults. Full reference: **[Contracts automation](./docs/contracts-automation.md)**. Managed with **`nodectl automation`** or `GET`/`POST /v1/automation/settings`.

#### `voting` (optional)

Automatic voting task configuration:

- `proposals` — list of tracked proposal ids (**64-character hex**, 32-byte hashes). Managed via **`nodectl vote add` / `vote rm`** (REST) or **`POST` / `DELETE /v1/voting/proposals`**
- `tick_interval` — interval between voting checks in seconds (default: `40`)

#### `log` (optional)

Logging configuration:

- `path` — log file path (optional; when `null`, file logging is disabled)
- `max_size_mb` — maximum log file size in MB before rotation (default: `50`)
- `max_files` — maximum number of rotated log files to keep (default: `10`)
- `rotation` — rotation frequency: `daily`, `hourly`, or `never` (default: `daily`)
- `level` — log level: `ERROR`, `WARN`, `INFO`, `DEBUG`, `TRACE` (default: `INFO`)
- `output` — log output target: `console`, `file`, or `all` (default: `console`)

### Default Config Example

```json
{
  "nodes": {},
  "wallets": {},
  "pools": {},
  "bindings": {},
  "ton_http_api": {
    "urls": [
      "http://127.0.0.1:3301/"
    ],
    "api_key": null
  },
  "elections": {
    "policy": "split50",
    "policy_overrides": {},
    "max_factor": 3.0,
    "tick_interval": 40
  },
  "http": {
    "bind": "0.0.0.0:8080",
    "enable_swagger": true
  },
  "master_wallet": {
    "key": {
      "name": "master-wallet-secret"
    },
    "version": "V3R2",
    "subwallet_id": 42,
    "workchain": 0
  },
  "tick_interval": 40,
  "log": {
    "max_size_mb": 50,
    "max_files": 10,
    "rotation": "daily",
    "level": "INFO",
    "output": "console"
  }
}
```

> **Tip:** Use `nodectl config generate` to create a default configuration file, then add nodes, wallets, pools, and bindings using `config node add`, `config wallet add`, `config pool add`, and `config bind add` commands.

---

## Service Mode (Daemon)

### Description

In service mode, nodectl runs as a daemon, automatically executing tasks on schedule.

### Elections Task

**Main task** — automatic participation in validator elections.

#### Algorithm:

1. **Check for active elections**
   - Query `get_active_election_id` to the Elector contract
   - If ID = 0, no elections — wait

2. **Get election parameters**
   - Configuration `#15` — election time parameters
   - Configuration `#34` — current validators
   - Query `past_elections` to the Elector contract

3. **For each enabled binding:**
   - **Stake recovery** — check and request return of frozen stake
   - **Stake calculation** - calculate round stake according to the stake policy
   - **Key generation** — create new validator key (if none exists)
   - **Bid formation** — prepare Election Bid
   - **Stake submission** — send transaction through wallet or pool

4. **Repeat** every N seconds

#### Nominator Pool Support

When a pool is present in the binding configuration:

- Transactions are sent to the pool contract
- The pool forwards funds to the Elector
- Stake is stored on the pool balance

#### Stake Policy

- `Split50` — split total available funds into two equal parts (default)
- `Minimum` — minimum stake for election participation
- `AdaptiveSplit50` — split half when above the Elector's estimated minimum effective stake, otherwise stake all (see [Staking Strategies](./docs/staking-strategies.md))
- `Fixed(amount)` — fixed amount in nanoTON

Each binding resolves its effective stake policy by checking for a per-node override first (`policy_overrides`); if none is set, the default `policy` is used. This allows running nodes with different stake strategies under a single configuration.

> **TONCore nominator caveat.** `Split50` and `AdaptiveSplit50` are ignored on bindings backed by a TONCore nominator — the two pools stake in different rounds, so there is nothing to split. The runner stakes the full liquid balance of the selected pool instead (still floored at `min_stake`). Use `Fixed` or `Minimum` if you need to cap per-round exposure on TONCore.

> **TONCore nominator: process pending withdraws before staking.** Every tick, the elections runner probes the active TONCore pool's `has_withdraw_requests` getter. When the queue is non-empty it sends `process_withdraw_requests` (op = 2, limit = 100) between `recover_stake` and `participate`, then skips this tick's stake submission to let the pool drain; the next tick re-probes and either resends op = 2 (new requests appeared) or proceeds to stake. This frees up locked liquidity from nominators who already requested withdrawal so it does not get re-staked. The corresponding participant status surfaced in the snapshot is `processing_withdraw_requests`. The step is a no-op for SNP and direct staking.

### Logging

Configure logging output and level in the config file (`log` section). Override the log level temporarily via environment variable:

```bash
RUST_LOG=debug nodectl service -c config.json
```

---

## Usage Examples

### Initial Setup

```bash
# Generate a default configuration file
nodectl config generate --output nodectl-config.json

# Define env var CONFIG_PATH=<path> to avoid explicit `--config <path>` argument in every command.

export CONFIG_PATH=./nodectl-config.json

# Generate vault keys
nodectl key add --name "node0-adnl-key" --extractable  # ADNL key must be extractable
nodectl key add --name "wallet0-key"                    # wallet key should NOT be extractable

# Add a node
nodectl config node add \
  --name node0 \
  --control-server-endpoint 192.168.1.100:50000 \
  --control-server-pubkey "kGViQgAAAAB..." \
  --control-client-secret-name "node0-adnl-key"

# Add a wallet
nodectl config wallet add \
  --name wallet0 \
  --secret-name "wallet0-key"

# Bind wallet to node
nodectl config bind add \
  --node node0 \
  --wallet wallet0

# Set TON HTTP API
nodectl config ton-http-api set \
  --endpoint "https://toncenter.com/api/v2/jsonRpc"

# Enable elections for the binding
nodectl config elections enable node0
```

### Setup with Nominator Pool

```bash
# Add a pool to the configuration
nodectl config pool add \
  --name pool0 \
  --owner "-1:owner_address"

# Bind wallet and pool to a node
nodectl config bind add \
  --node node0 \
  --wallet wallet0 \
  --pool pool0

# Deploy the pool contract (binding must reference pool0)
nodectl deploy pool \
  --config my-config.json \
  --binding my-binding \
  --owner "-1:owner_address" \
  --amount 1.5
```

### Configuration Management

```bash
# List all nodes, wallets, pools, bindings
nodectl config node ls
nodectl config wallet ls
nodectl config pool ls
nodectl config bind ls

# View log configuration
nodectl config log ls

# Set log level and output mode
nodectl config log set --level debug --output file --path /var/log/nodectl/node.log

# View elections configuration
nodectl config elections show

# Set default stake policy
nodectl config elections stake-policy --minimum

# Set adaptive split50
nodectl config elections stake-policy --adaptive-split50

# Override policy for a specific node
nodectl config elections stake-policy --node node0 --fixed 500

# Remove a per-node override
nodectl config elections stake-policy --node node0 --reset

# Set elections tick interval
nodectl config elections tick-interval 60

# Set max factor
nodectl config elections max-factor 2.5

# AdaptiveSplit50 timing (fractions of election duration, [0.0, 1.0])
nodectl config elections wait --min 0.15 --max 0.45
```

### Authentication Setup

```bash
# Create an operator user
nodectl auth add --username admin --role operator

# Create a read-only nominator user
nodectl auth add --username viewer --role nominator

# List users
nodectl auth ls

# Configure token TTL
nodectl auth set ttl --operator 8h --nominator 1h

# Log in and obtain a JWT token
nodectl api login admin

# Non-interactive login (for scripts)
echo "$PASSWORD" | nodectl api login admin --password-stdin

# Export the token for subsequent commands
export NODECTL_API_TOKEN="<token>"

# Revoke all tokens for a user
nodectl auth revoke admin

# Remove a user
nodectl auth rm viewer
```

### Key Management

```bash
# List all vault keys
nodectl key ls

# Import an existing key
nodectl key import \
  --name "imported-key" \
  --private-key "base64-private-key" \
  --extractable

# Remove a key
nodectl key rm --name "old-key"
```

### Get Configuration Parameters (CLI mode)

```bash
# Get config param #34 (current validators)
nodectl config-param 34

# Get config param #15 (election parameters)
nodectl config-param 15
```

### Deploy Wallets and Pools

```bash
# Deploy wallet for a specific node
nodectl deploy wallet --node node0 --verbose

# Deploy all wallets
nodectl deploy wallet --all --verbose

# Deploy a Single Nominator Pool
nodectl deploy pool \
  --config config.json \
  --binding my-binding \
  --owner "-1:owner_address" \
  --amount 1.5
```

### Send TON

```bash
# Send TON from a wallet
nodectl config wallet send \
  --from wallet0 \
  --to "-1:destination_address" \
  --amount 10.0
```

### Manual staking

`nodectl config wallet stake` sends an election stake through a nominator pool. Use it to participate in elections manually.

```bash
nodectl config wallet stake -b <BINDING> -a <AMOUNT> [-m <MAX_FACTOR>] [--pool-even | --pool-odd]
```

| Flag | Long | Required | Default | Description |
|------|------|----------|---------|-------------|
| `-b` | `--binding` | Yes | — | Binding name (node-wallet-pool triple) |
| `-a` | `--amount` | Yes | — | Stake amount in TON |
| `-m` | `--max-factor` | No | `3.0` | Max factor: from `1.0` up to the network limit (**config param 17**), validated against the chain |
|      | `--pool-even` | No | (default if neither flag is set) | TONCore only: use the pool for even validation rounds |
|      | `--pool-odd` | No | — | TONCore only: use the pool for odd validation rounds |

Example:

```bash
nodectl config wallet stake -b node0 -a 50000 -m 2.5
```

The command validates that elections are active, manages validator keys and ADNL addresses automatically, builds and sends the stake transaction, and polls the Elector until the stake is confirmed.

### Starting the Service Daemon

```bash
# Start service (foreground) - requires config file
nodectl service

# With debug logging (via env)
RUST_LOG=debug nodectl service
```

### Managing Elections via API

```bash
# Check service health
nodectl api health

# Get elections status
nodectl api elections

# Exclude a node from elections
nodectl api elections --exclude node0

# Include a node back
nodectl api elections --include node0

# Get validators info
nodectl api validators
```

### Controlling Background Tasks

```bash
# Disable elections task
nodectl api task elections disable

# Enable elections task
nodectl api task elections enable

# Restart elections task
nodectl api task elections restart
```

### Setting Stake Policy (Runtime)

Change stake policy on a running service (via API):

```bash
# Use minimum stake (default for all nodes)
nodectl api stake-policy --minimum

# Use fixed stake (1000 TON)
nodectl api stake-policy --fixed 1000000000000

# Use 50% of available balance
nodectl api stake-policy --split50

# Use adaptive split50
nodectl api stake-policy --adaptive-split50

# Override policy for a specific node
nodectl api stake-policy --node node0 --fixed 500000000000
```

### Using REST API Directly

```bash
# Login and obtain a token
TOKEN=$(curl -s -X POST http://127.0.0.1:8080/auth/login \
  -H "Content-Type: application/json" \
  -d '{"username": "admin", "password": "secret"}' | jq -r '.token')

# Health check (public, no token required)
curl http://127.0.0.1:8080/health

# Get elections
curl http://127.0.0.1:8080/v1/elections \
  -H "Authorization: Bearer $TOKEN"

# Get validators
curl http://127.0.0.1:8080/v1/validators \
  -H "Authorization: Bearer $TOKEN"

# Exclude nodes
curl -X POST http://127.0.0.1:8080/v1/elections/exclude \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"nodes": ["node0"]}'

# Set default stake policy
curl -X POST http://127.0.0.1:8080/v1/elections/settings \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"policy": "minimum"}'

# Set per-node policy override
curl -X POST http://127.0.0.1:8080/v1/elections/settings \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"policy": {"fixed": 500000000000}, "node": "node0"}'

# Update elections tick interval and max factor in one call
curl -X POST http://127.0.0.1:8080/v1/elections/settings \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"tick_interval": 60, "max_factor": 2.5}'

# Replace TON HTTP API endpoint list
curl -X POST http://127.0.0.1:8080/v1/ton-http-api \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"urls": ["https://toncenter.com/api/v2/jsonRpc"]}'

# Add a wallet
curl -X POST http://127.0.0.1:8080/v1/wallets \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"name": "wallet0", "secret": "wallet0-key", "version": "V3R2", "subwallet_id": 42, "workchain": -1}'

# Control elections task
curl -X POST http://127.0.0.1:8080/v1/task/elections \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"action": "restart"}'
```

---

## Related Setup Guides

- [Hashicorp Vault Dedicated Setup](./docs/hcp-vault-setup.md)
- [Node Control Service Setup](./docs/nodectl-setup.md)
- [Contracts automation (auto-deploy / auto-topup)](./docs/contracts-automation.md) — `automation` config, REST and CLI
- [Security Guide](./docs/nodectl-security.md) — roles, token lifecycle, rate limiting, monitoring
