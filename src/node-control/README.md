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
- **Pool support** — work through validator wallet or nominator pool (single-nominator)
- **Flexible stake policies** — minimum, fixed, or split50 stake strategies with per-node overrides
- **Swagger UI** — interactive API documentation

### Features roadmap

|   Feature   | Status | Comment |
|-------------|------------|-------------|
| Automatic elections | Done | - |
| Nominator Pools support | Done | Single Nominator Pool only |
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

> **Note:** The `--config` (`-c`) flag is specified per subcommand (e.g. `nodectl service -c config.json`, `nodectl config-param -c config.json 34`), not globally.

---

## Commands

To enable detailed logging output, use the RUST_LOG environment variable (available log levels: error, warn, info, debug, trace):
```bash
RUST_LOG=debug nodectl ...
```

### Configuration Commands

Commands for managing nodectl configuration files. All `config` subcommands accept a global `--config` (`-c`) flag to specify the configuration file path (default: `nodectl-config.json`). This can also be set via the `CONFIG_PATH` environment variable.

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

Manage nominator pools in the configuration file.

##### `config pool add`

Add a Single Nominator Pool to the configuration. Pools can be added with an existing contract address (already deployed) or with an owner address (for future deployment).

| Flag | Short form | Description |
|------|------------|-------------|
| `--name <NAME>` | `-n` | Pool name (unique identifier) |
| `--address <ADDRESS>` | `-a` | Pool contract address (if already deployed, optional) |
| `--owner <ADDRESS>` | `-o` | Owner address for deployment/verification (optional) |

```bash
# Add a pool with a known address
nodectl config pool add \
  --name pool0 \
  --address "-1:pool_contract_address"

# Add a pool with owner for future deployment
nodectl config pool add \
  --name pool0 \
  --owner "-1:owner_address"
```

##### `config pool ls`

List all configured pools.

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

Set the TON HTTP API URL and optional API key.

| Flag | Short form | Description |
|------|------------|-------------|
| `--url <URL>` | `-u` | TON HTTP API URL |
| `--api-key <KEY>` | `-k` | API key (optional) |

```bash
# Set TON HTTP API URL
nodectl config ton-http-api set --url "https://toncenter.com/api/v2/jsonRpc"

# Set with API key
nodectl config ton-http-api set \
  --url "https://toncenter.com/api/v2/jsonRpc" \
  --api-key "your-api-key"
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

Manage elections configuration, including stake policies, tick intervals, and per-binding election participation.

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
| `--node <NAME>` | `-n` | Apply policy only to this node (override). Omit to set the default policy. |
| `--reset` | | Remove a per-node policy override (requires `--node`) |

```bash
# Set default policy to minimum stake
nodectl config elections stake-policy --minimum

# Set fixed stake (1000 TON)
nodectl config elections stake-policy --fixed 1000

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

Set the maximum factor for elections. Must be in the range [1.0..3.0].

| Argument | Description |
|----------|-------------|
| `<VALUE>` | Max factor value |

```bash
nodectl config elections max-factor 2.5
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

---

#### `config stake-policy`

Shortcut for `config elections stake-policy`. Set the stake policy in the configuration file. By default the policy applies to **all nodes**. Use `--node` to set a per-node override that takes precedence over the default.

| Flag | Short form | Description |
|------|------------|-------------|
| `--fixed <AMOUNT>` | | Fixed stake amount in TON |
| `--split50` | | Use 50% of available balance |
| `--minimum` | | Use minimum required stake |
| `--node <NAME>` | `-n` | Apply policy only to this node (per-node override). Omit to set the default policy for all nodes. |
| `--reset` | | Remove a per-node policy override (requires `--node`) |

```bash
# Set minimum stake policy (default for all nodes)
nodectl config stake-policy --minimum

# Set fixed stake (1000 TON)
nodectl config stake-policy --fixed 1000

# Set split50 policy
nodectl config stake-policy --split50

# Override policy for a specific node
nodectl config stake-policy --node node0 --fixed 500

# Remove a per-node override (node falls back to default policy)
nodectl config stake-policy --node node0 --reset
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

Deploy a Single Nominator Pool contract.

| Option | Short form | Description |
|--------|------------|-------------|
| `--config <FILE>` | `-c` | Path to the configuration file. Can also be set as an environment variable CONFIG_PATH |
| `--node <NAME>` | | Node ID (the wallet of this node is used to deploy the pool) |
| `--owner <ADDRESS>` | | Address of the pool owner |
| `--amount <TON>` | | Amount of TON to transfer to the pool contract for deployment |
| `--verbose` | | Print deployment progress |

```bash
nodectl deploy pool \
  --config config.json \
  --node node0 \
  --owner "-1:owner_address_here" \
  --amount 1.5
```

The command calculates the pool address from the owner and validator wallet, sends a deploy message with the specified amount, and waits for the contract to become active. The result is printed as JSON with the pool address and deployment status.

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

Set the stake policy for elections on a running service. Use `--node` to set a per-node override instead of changing the default policy.

| Flag | Short form | Description |
|------|------------|-------------|
| `--fixed <AMOUNT>` | | Fixed stake amount (in nanoTON) |
| `--split50` | | Use 50% of available balance |
| `--minimum` | | Use minimum required stake |
| `--node <NAME>` | `-n` | Apply policy only to this node (per-node override). Omit to set the default policy. |

```bash
# Set minimum stake policy (default for all nodes)
nodectl api stake-policy --minimum

# Set fixed stake (1000 TON = 1000000000000 nanoTON)
nodectl api stake-policy --fixed 1000000000000

# Set split50 policy
nodectl api stake-policy --split50

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

#### `GET /health`

Health check endpoint.

**Response:**

```json
{
  "ok": true,
  "result": "OK"
}
```

---

#### `POST /auth/login`

Authenticate and obtain a JWT token. Rate-limited: 5 failed attempts per 60s window, then blocked for 120s.

**Request:**

```json
{
  "username": "admin",
  "password": "secret"
}
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

Return the identity of the authenticated user. Requires: `nominator` or `operator` role.

**Response:**

```json
{
  "ok": true,
  "username": "admin",
  "role": "operator"
}
```

---

#### `GET /auth/users`

List all users. Requires: `operator` role.

**Response:**

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

Get current elections snapshot. Requires: `nominator` or `operator` role.

**Response:**

```json
{
  "ok": true,
  "result": {
    "election_id": 1734523200,
    "elect_close": 1734522300,
    "min_stake": 300000000000000,
    "total_stake": 15000000000000000,
    "participants": [...],
    "failed": false,
    "finished": false
  }
}
```

---

#### `POST /v1/elections/exclude`

Exclude nodes from elections participation.

**Request:**

```json
{
  "nodes": ["node0", "node1"]
}
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

Include nodes back into elections participation.

**Request:**

```json
{
  "nodes": ["node0"]
}
```

**Response:**

```json
{
  "ok": true,
  "result": {
    "excluded": ["node1"],
    "updated_at": 1734523200
  }
}
```

---

#### `GET /v1/validators`

Get current validators snapshot.

**Response:**

```json
{
  "ok": true,
  "result": {
    "validators": [...],
    "utime_since": 1734400000,
    "utime_until": 1734486400
  }
}
```

---

#### `POST /v1/stake_strategy`

Set the stake policy for elections. Optionally include a `node` field to apply the policy as a per-node override instead of changing the default.

**Request (default policy — minimum stake):**

```json
{
  "policy": "minimum"
}
```

**Request (default policy — fixed stake):**

```json
{
  "policy": { "fixed": 1000000000000 }
}
```

**Request (default policy — split50):**

```json
{
  "policy": "split50"
}
```

**Request (per-node override):**

```json
{
  "policy": { "fixed": 500000000000 },
  "node": "node0"
}
```

**Response:**

```json
{
  "ok": true,
  "result": {
    "policy": "minimum",
    "applied_at": 1734523200
  }
}
```

**Response (per-node override):**

```json
{
  "ok": true,
  "result": {
    "policy": { "fixed": 500000000000 },
    "node": "node0",
    "applied_at": 1734523200
  }
}
```

---

#### `POST /v1/task/elections`

Control the elections background task.

**Request:**

```json
{
  "action": "enable" | "disable" | "restart"
}
```

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
    "url": "http://127.0.0.1:3301/",
    "api_key": "<OPTIONAL_API_KEY>" | null
  },
  "http": {
    "bind": "0.0.0.0:8080",
    "enable_swagger": true,
    "api_key": null
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
    "policy": "split50" | "minimum" | { "fixed": 1000000000000 },
    "policy_overrides": { "<node_name>": "minimum" | { "fixed": <amount> } | "split50" },
    "max_factor": 3.0,
    "tick_interval": 40
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

Nominator pool configurations. Two pool types are supported:

**Single Nominator Pool (SNP):**

- `kind` — `"snp"`
- `address` — deployed pool contract address (optional)
- `owner` — pool owner address (optional)

**TONCore Pool:**

- `kind` — `"core"`
- `addresses` — two addresses: validator wallet (`[0]`) and pool contract (`[1]`, must match the address derived from the parameters below)
- `validator_share` — validator reward share (basis points; stored as `u16` on-chain)
- `max_nominators` — optional; if omitted, `contracts` `resolve_deploy_pool_params` uses the default next to the pool contract
- `min_validator_stake` — optional (nanotons); same
- `min_nominator_stake` — optional (nanotons); same

#### `bindings`

Bindings link nodes to wallets and pools for elections participation:

- `wallet` — wallet name (must reference a key in `wallets`)
- `pool` — pool name (optional, must reference a key in `pools`)
- `enable` — whether this binding participates in elections (default: `false`)
- `status` — current binding status: `idle`, `participating`, `draining`, `validating` (managed automatically)

#### `ton_http_api`

TON HTTP API configuration:

- `url` — JSON-RPC endpoint URL (default: `http://127.0.0.1:3301/`)
- `api_key` — API key (optional)

#### `http`

HTTP REST API server configuration:

- `bind` — address and port to bind (default: `0.0.0.0:8080`)
- `enable_swagger` — enable Swagger UI at `/swagger` (default: `true`)
- `auth` — JWT authentication configuration (see below)

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
  - `{ "fixed": <amount> }` — fixed stake amount in nanoTON
- `policy_overrides` — per-node stake policy overrides (node name -> policy). When a node has an entry here, it takes precedence over the default `policy`. Example: `{ "node0": { "fixed": 500000000000 } }`
- `max_factor` — max factor for elections (default: 3.0, must be in range [1.0..3.0])
- `tick_interval` — interval between election checks in seconds (default: `40`)

#### `voting` (optional)

Automatic voting task configuration:

- `proposals` — list of proposal addresses to vote for
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
- `Fixed(amount)` — fixed amount in nanoTON

Each binding resolves its effective stake policy by checking for a per-node override first (`policy_overrides`); if none is set, the default `policy` is used. This allows running nodes with different stake strategies under a single configuration.

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
  --url "https://toncenter.com/api/v2/jsonRpc"

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

# Deploy the pool contract
nodectl deploy pool \
  --config my-config.json \
  --node node0 \
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

# Set default stake policy in config
nodectl config stake-policy --minimum

# Override policy for a specific node
nodectl config stake-policy --node node0 --fixed 500

# Remove a per-node override
nodectl config stake-policy --node node0 --reset

# Set elections tick interval
nodectl config elections tick-interval 60

# Set max factor
nodectl config elections max-factor 2.5
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
  --node node0 \
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
nodectl config wallet stake -b <BINDING> -a <AMOUNT> [-m <MAX_FACTOR>]
```

| Flag | Long | Required | Default | Description |
|------|------|----------|---------|-------------|
| `-b` | `--binding` | Yes | — | Binding name (node-wallet-pool triple) |
| `-a` | `--amount` | Yes | — | Stake amount in TON |
| `-m` | `--max-factor` | No | `3.0` | Max factor (`1.0`–`3.0`) |

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
curl -X POST http://127.0.0.1:8080/v1/stake_strategy \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"policy": "minimum"}'

# Set per-node policy override
curl -X POST http://127.0.0.1:8080/v1/stake_strategy \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"policy": {"fixed": 500000000000}, "node": "node0"}'

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
- [Security Guide](./docs/nodectl-security.md) — roles, token lifecycle, rate limiting, monitoring
