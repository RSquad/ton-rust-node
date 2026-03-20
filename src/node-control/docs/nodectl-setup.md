# TON Node Control Tool — Setup Guide

This guide provides step-by-step instructions for deploying and configuring **nodectl** — a utility for managing TON validator nodes with automatic elections participation.

**Table of Contents**

- [Prerequisites](#prerequisites)
- [Step 1: Install nodectl](#step-1-install-nodectl)
- [Step 2: Configure SecretsVault](#step-2-configure-secretsvault)
- [Step 3: Create nodectl Configuration](#step-3-create-nodectl-configuration)
- [Step 4: Configure TON HTTP API](#step-4-configure-ton-http-api)
- [Step 5: Master Wallet](#step-5-master-wallet)
- [Step 6: Add Nodes](#step-6-add-nodes)
- [Step 7: Add Wallets](#step-7-add-wallets)
- [Step 8: Add Pools](#step-8-add-pools)
- [Step 9: Create Secrets in Vault](#step-9-create-secrets-in-vault)
- [Step 10: Configure Control Server Client Keys](#step-10-configure-control-server-client-keys)
- [Step 11: Create Bindings](#step-11-create-bindings)
- [Step 12: Configure Logging](#step-12-configure-logging)
- [Step 13: Increase non-swap memory limits](#step-13-increase-non-swap-memory-limits)
- [Step 14: Run the Service](#step-14-run-the-service)
- [Step 15: Enable Elections](#step-15-enable-elections)
- [Security Recommendations](#security-recommendations)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

> **Note:** For an overview of the architecture and capabilities of nodectl, see [README.md](../README.md).

Before starting the deployment, ensure you have:

| Requirement | Description |
|-------------|-------------|
| **Docker** | Docker Engine installed |
| **TON Node(s)** | Running TON Rust validator nodes |

---

## Step 1: Install nodectl

nodectl is distributed as a Docker image. Pull the latest version:

```bash
docker pull ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.1
```

To run any `nodectl` CLI command, use `docker run` with the image:

```bash
docker run --rm \
  -v "$(pwd)/nodectl-config.json":/nodectl/config.json \
  -e VAULT_URL="$VAULT_URL" \
  -e CONFIG_PATH="/nodectl/config.json" \
  ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.1 \
  nodectl <command> [options]
```

For convenience, create a shell alias:

```bash
alias nodectl='docker run --rm \
  -v "$(pwd)/nodectl-config.json":/nodectl/config.json \
  -e VAULT_URL="$VAULT_URL" \
  -e CONFIG_PATH="/nodectl/config.json" \
  ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.1 \
  nodectl'
```

> **Note (file-based vault only):** If you are using the `file://` vault backend, the vault file must also be mounted into the container, otherwise it will be lost when the container exits. Extend the alias with an extra volume mount:
> ```bash
> alias nodectl='docker run --rm \
>   -v "$(pwd)/nodectl-config.json":/nodectl/config.json \
>   -v "$(pwd)/vault.json":/nodectl/vault.json \
>   -e VAULT_URL="file:///nodectl/vault.json?master_key=$MASTER_KEY" \
>   -e CONFIG_PATH="/nodectl/config.json" \
>   ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.1 \
>   nodectl'
> ```

Now you can use `nodectl` as if it were installed locally:

```bash
nodectl --version
```

> **Note:** All examples in this guide assume the alias is set up. If you don't use the alias, prepend the full `docker run ...` command to each `nodectl` invocation.

---

## Step 2: Configure SecretsVault

nodectl stores private keys (validator wallet keys, ADNL client keys) in a secure storage — **SecretsVault**. SecretsVault abstracts the storage backend behind a unified API for key management, regardless of how secrets are stored physically.

Two backends are supported:

- **File-based** — secrets are stored in an encrypted JSON file on disk, protected by a master key.
- **HashiCorp Vault** — secrets are stored in HashiCorp Vault using the Transit secrets engine.

The backend is selected via the `VAULT_URL` environment variable. Choose one of the options below and set the variable before proceeding.

### 2.1 File-based backend

**URL format:**

```
file://<path-to-vault-file>?master_key=<hex-encoded-master-key>
```

Generate a random master key (32 bytes hex):

```bash
export MASTER_KEY=$(openssl rand -hex 32)
```

Set the `VAULT_URL` environment variable:

```bash
export VAULT_URL="file://vault.json?master_key=$MASTER_KEY"
```

The vault file will be created automatically on first use. Keep the master key safe — without it the vault file cannot be decrypted.

### 2.2 HashiCorp Vault backend

For detailed instructions on setting up HashiCorp Cloud Platform (HCP) Vault Dedicated, see: **[HCP Vault Setup Guide](./hcp-vault-setup.md)**

Once Vault is set up, you should have the following environment variables:

```bash
export VAULT_ADDR="<your-public-cluster-URL>"
export VAULT_NAMESPACE="admin/nodectl"
export VAULT_TOKEN="<your-admin-token>"
```

Set the `VAULT_URL` environment variable:

```bash
export VAULT_URL="hashicorp://$VAULT_ADDR?api_key=$VAULT_TOKEN&namespace=$VAULT_NAMESPACE"
```

---

## Step 3: Create nodectl Configuration

Generate an empty default configuration file:

```bash
# Create a placeholder file first — Docker requires the host file to exist before the
# first `docker run`. Without it, Docker creates a directory named nodectl-config.json
# instead of a file bind-mount, and the config generate command will fail.
touch nodectl-config.json
nodectl config generate -o /nodectl/config.json
```

The `-o /nodectl/config.json` flag writes the config to the path mounted by the Docker alias. Without it, `nodectl config generate` would write to `nodectl-config.json` inside the ephemeral container and the file would be lost on exit.

> **Note:** All subsequent `nodectl config` commands accept `-c <path>` (or `--config <path>`) to specify the config file. Default is `nodectl-config.json`. You can also set the `CONFIG_PATH` environment variable.

---

## Step 4: Configure TON HTTP API

nodectl needs access to the TON blockchain to read on-chain data (elections, validator sets, account states). It connects through the JSON RPC server running on a TON fullnode.

Set the RPC server URL:

```bash
nodectl config ton-http-api set -u "http://127.0.0.1:3301/"
```

With an optional API key:

```bash
nodectl config ton-http-api set -u "http://127.0.0.1:3301/" -k "your-api-key"
```

> **Important**: Do not enable the RPC server on a validator node. Use a separate TON fullnode as the RPC server.

Make sure the TON node's `config.json` has the RPC server enabled:

```json
{
    "json_rpc_server": {
        "address": "127.0.0.1:3301"
    }
}
```

---

## Step 5: Master Wallet

The **master wallet** is a central funding source that the service uses to:

- Automatically deploy validator wallets and nominator pools
- Periodically top up validator wallets when their balance drops below the threshold (5 TON)

When you created the configuration file in [Step 3](#step-3-create-nodectl-configuration), a `master_wallet` section was included automatically. The `key.name` field contains the vault secret name. If the secret does not exist in the vault yet, the service will **automatically generate** it on first startup.

View master wallet information:

```bash
nodectl config master-wallet info
```

This shows the wallet address, balance, state, and public key.

**Fund the master wallet address** with enough TON to cover deployment costs and ongoing wallet top-ups. Each wallet/pool deployment costs ~1 TON, and the service sends 10 TON when topping up wallets.

> **Important:** Keep the master wallet balance above 10 TON. The service will periodically check balances and send top-ups from the master wallet automatically, but it cannot fund itself.

---

## Step 6: Add Nodes

Add each TON validator node to the configuration. For each node you need three things:

- **Control Server endpoint** — the IP address and port where the node's Control Server is listening (e.g. `192.168.1.10:3031`). You can find this in the node's `config.json` under `control_server.address`.
- **Control Server public key** — the server's public key in Base64 format. You can find it in the node's `config.json` under `control_server.server_key` (derive the public key from the private key, or use the key provided during node setup).
- **Client secret name** — the name of the vault secret for the ADNL client private key. This key will be created later in [Step 9](#step-9-create-secrets-in-vault). For now, just choose a name (e.g. `control-client-secret`).

```bash
nodectl config node add \
    -n node1 \
    -e "192.168.1.10:3031" \
    -p "BK6eLCiiIvKKBH3qCZ7tX1in3UDdfYMBitmUKUbf36M=" \
    -s "control-client-secret"
```

**Options:**

| Option | Description |
|--------|-------------|
| `-n, --name` | Unique node name |
| `-e, --control-server-endpoint` | Control Server address (IP:PORT) |
| `-p, --control-server-pubkey` | Control Server public key (Base64) |
| `-s, --control-client-secret-name` | Vault secret name for ADNL client private key |

All nodes can share the same client key secret name (e.g. `control-client-secret`) — a single ADNL key can authenticate with multiple nodes.

Repeat for each node:

```bash
nodectl config node add -n node2 -e "192.168.1.11:3031" -p "<SERVER_PUBKEY_BASE64>" -s "control-client-secret"
nodectl config node add -n node3 -e "192.168.1.12:3031" -p "<SERVER_PUBKEY_BASE64>" -s "control-client-secret"
```

List configured nodes:

```bash
nodectl config node ls
```

---

## Step 7: Add Wallets

Add a validator wallet for each node. Each wallet needs a unique name and a vault secret name for its private key. The key will be created in [Step 9](#step-9-create-secrets-in-vault).

```bash
nodectl config wallet add -n wallet1 -s "wallet1-secret"
```

**Options:**

| Option | Description |
|--------|-------------|
| `-n, --name` | Unique wallet name |
| `-s, --secret-name` | Vault secret name for wallet private key |
| `-v, --version` | Wallet version: `V3R2` (default), `V4R2`, `V5R1` |
| `-w, --workchain` | Workchain ID (default: `-1`) |
| `-i, --subwallet-id` | Subwallet ID (default: `42`) |

Example for multiple nodes:

```bash
nodectl config wallet add -n wallet1 -s "wallet1-secret"
nodectl config wallet add -n wallet2 -s "wallet2-secret"
nodectl config wallet add -n wallet3 -s "wallet3-secret"
```

List configured wallets:

```bash
nodectl config wallet ls
```

---

## Step 8: Add Pools

Add a Single Nominator Pool for each validator:

```bash
nodectl config pool add -n pool1 -o "0:bd313e9e1114bbbe7af6f28ef59be0ff3f02ac795423f10397a70dc16396c4ea"
```

**Options:**

| Option | Description |
|--------|-------------|
| `-n, --name` | Unique pool name |
| `-o, --owner` | Owner address (nominator address) |
| `-a, --address` | Pool contract address (if already deployed) |

Example for multiple pools:

```bash
nodectl config pool add -n pool1 -o "0:<OWNER_ADDRESS_1>"
nodectl config pool add -n pool2 -o "0:<OWNER_ADDRESS_2>"
nodectl config pool add -n pool3 -o "0:<OWNER_ADDRESS_3>"
```

List configured pools:

```bash
nodectl config pool ls
```

---

## Step 9: Create Secrets in Vault

Now create the cryptographic keys that you referenced in the previous steps. These keys are stored in the vault and used by nodectl for signing transactions and authenticating with nodes.

Make sure the `VAULT_URL` environment variable is set (see [Step 2](#step-2-configure-secretsvault)).

```bash
# Create the control client key (shared by all nodes)
# Must be --extractable because nodectl reads the private key for ADNL connections
nodectl key add -n "control-client-secret" -e

# Create wallet keys (one per wallet)
nodectl key add -n "wallet1-secret"
nodectl key add -n "wallet2-secret"
nodectl key add -n "wallet3-secret"
```

**Options:**

| Option | Description |
|--------|-------------|
| `-n, --name` | Secret name (must match what you used in config) |
| `-a, --algorithm` | Key algorithm (default: `ed25519`) |
| `-e, --extractable` | Allow private key extraction (required for ADNL client keys) |

> **Note:** Wallet keys should **not** be extractable. Only control client keys need the `-e` flag.

Import an existing private key instead of generating a new one:

```bash
nodectl key import -n "my-key" -k "<base64-encoded-private-key>" -e
```

List all secrets in the vault:

```bash
nodectl key ls
```

The output shows the name, algorithm, extractable flag, creation date, and **public key** for each secret.

---

## Step 10: Configure Control Server Client Keys

Now that the control client key is created, you need to register its public key on each TON node. This allows nodectl to authenticate and send commands to the nodes.

### 10.1 Get the Client Public Key

Run the following command and find the row with your control client secret name (e.g. `control-client-secret`):

```bash
nodectl key ls
```

Copy the **Public Key** value (Base64-encoded).

### 10.2 Add Client Key to Node Config

In each TON node's configuration file (`config.json`), add the client public key to the `clients` list inside the `control_server` section:

```json
{
  "control_server": {
    "address": "0.0.0.0:3031",
    "server_key": {
      "type_id": 1209251014,
      "pvt_key": "<SERVER_PRIVATE_KEY_BASE64>"
    },
    "clients": {
      "list": [
        {
          "type_id": 1209251014,
          "pub_key": "<CLIENT_PUBLIC_KEY_BASE64>"
        }
      ]
    }
  }
}
```

Replace `<CLIENT_PUBLIC_KEY_BASE64>` with the public key obtained from `nodectl key ls`.

If you use the same client secret for all nodes, the same public key goes into every node's config. `type_id` is always `1209251014` (Ed25519).

Restart each TON node after modifying its `config.json`.

---

## Step 11: Create Bindings

Bindings connect a node to its wallet and (optionally) a pool. This tells the service which wallet and pool to use for elections on each node.

```bash
nodectl config bind add -n node1 -w wallet1 -p pool1
nodectl config bind add -n node2 -w wallet2 -p pool2
nodectl config bind add -n node3 -w wallet3 -p pool3
```

**Options:**

| Option | Description |
|--------|-------------|
| `-n, --node` | Node name (must exist in config) |
| `-w, --wallet` | Wallet name (must exist in config) |
| `-p, --pool` | Pool name (optional, must exist in config) |

List all bindings:

```bash
nodectl config bind ls
```

---

## Step 12: Configure Logging

Logging is configured directly in the config file under the `log` section:

```json
{
    "log": {
        "level": "INFO",
        "output": "all",
        "path": "./logs/nodectl.log",
        "max_size_mb": 50,
        "max_files": 10,
        "rotation": "daily"
    }
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `level` | `INFO` | Log level: `TRACE`, `DEBUG`, `INFO`, `WARN`, `ERROR` |
| `output` | `console` | Where to write: `console`, `file`, or `all` (both) |
| `path` | — | Log file path (required if `output` is `file` or `all`) |
| `max_size_mb` | `50` | Max size of a single log file in MB before rotation |
| `max_files` | `10` | Number of rotated log files to keep |
| `rotation` | `daily` | Rotation schedule: `daily`, `hourly`, or `never` |

**Production example:**

```json
{
    "log": {
        "level": "INFO",
        "output": "all",
        "path": "/var/log/nodectl/service.log",
        "max_size_mb": 100,
        "max_files": 20,
        "rotation": "daily"
    }
}
```

---

## Step 13: Increase non-swap memory limits

### Configuring Locked Memory Limit

The crypto-vault application uses `mlock()` to prevent sensitive data from being swapped to disk. This requires sufficient locked memory limits.

#### Check Current Limit

```bash
ulimit -l
```

Output shows the limit in **kilobytes**. If the value is less than `1024` (1 MB), increase it as follows.

#### Update Limit to 4 GB

1. Open the limits configuration file:

```bash
sudo nano /etc/security/limits.conf
```

2. Add these lines at the end (replace `your_username` with your actual username):

```
your_username soft memlock 4194304
your_username hard memlock 4194304
```

> To apply for all users, use `*` instead of username.

3. Save and close the file.

4. Log out and log back in, then verify:

```bash
ulimit -l
```

Expected output: `4194304`

---

## Step 14: Run the Service

```bash
# Create logs directory if it doesn't exist
mkdir -p "$(pwd)/logs"

docker run -d \
  --name nodectl --restart unless-stopped \
  -v "$(pwd)/logs":/nodectl/logs \
  -v "$(pwd)/nodectl-config.json":/nodectl/config.json \
  -e VAULT_URL="$VAULT_URL" \
  -e CONFIG_PATH="/nodectl/config.json" \
  -e RUST_BACKTRACE=1 \
  ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.1 \
  nodectl service --config=/nodectl/config.json
```

> **Note (file-based vault only):** If you are using the `file://` vault backend, add a volume mount for the vault file so it persists across container restarts:
> ```bash
> docker run -d \
>   --name nodectl --restart unless-stopped \
>   -v "$(pwd)/logs":/nodectl/logs \
>   -v "$(pwd)/nodectl-config.json":/nodectl/config.json \
>   -v "$(pwd)/vault.json":/nodectl/vault.json \
>   -e VAULT_URL="file:///nodectl/vault.json?master_key=$MASTER_KEY" \
>   -e CONFIG_PATH="/nodectl/config.json" \
>   -e RUST_BACKTRACE=1 \
>   ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.1 \
>   nodectl service --config=/nodectl/config.json
> ```
> Without this mount, all vault keys (wallet keys, ADNL keys) will be lost on every container restart.

### What the Service Does

Once started, the service:

1. **Hot-reloads configuration** — checks the config file every **10 seconds** and applies changes without restart. You can edit the config or use `nodectl config` commands while the service is running.

2. **Auto-deploys contracts** — automatically deploys validator wallets and nominator pools that are configured but not yet deployed on-chain. Deployment is funded from the master wallet.

3. **Auto-funds wallets** — periodically checks validator wallet balances and sends **10 TON** from the master wallet when a wallet balance drops below **5 TON**.

> **Important:** Keep the master wallet address funded. If its balance drops below 10 TON, the service will not be able to top up validator wallets or deploy new contracts. Monitor it with `nodectl config master-wallet info`.

---

## Step 15: Enable Elections

By default, elections participation is **disabled** for all bindings. You must explicitly enable it.

### 15.1 Enable Elections

```bash
# Enable elections for specific bindings (by node name)
nodectl config elections enable node1 node2 node3
```

### 15.2 Disable Elections

```bash
nodectl config elections disable node1
```

### 15.3 View Election Status

```bash
nodectl config elections show
```

Use `--format json` for machine-readable output:

```bash
nodectl config elections show --format json
```

### 15.4 Binding Statuses

Each binding has a `status` field that the service manages automatically. The following statuses exist:

| Status | Description |
|--------|-------------|
| **`idle`** | Default state. Elections are disabled or no activity. |
| **`participating`** | Elections are enabled, stake has been submitted, waiting for validator set. |
| **`validating`** | Node is in the current validator set. |
| **`draining`** | Elections were disabled or node left the validator set; waiting for recovery stake to be returned. |

**Status transitions** (managed automatically by the service):

```
idle -> participating    (elections enabled, elections round is open)
participating -> validating  (node enters the validator set)
validating -> draining       (elections disabled, or node leaves the validator set with pending recovery)
draining -> idle             (recovery stake fully returned)
validating -> idle           (node leaves the validator set, no pending recovery)
```

The service updates binding statuses in the config file automatically. You can observe them via `nodectl config elections show` or by inspecting the config file directly.

### 15.5 Configure Stake Policy

```bash
# Minimum required stake
nodectl config elections stake-policy --minimum

# Fixed amount in TON
nodectl config elections stake-policy --fixed 100

# Split available balance 50/50 (default)
nodectl config elections stake-policy --split50

# Override policy for a specific node
nodectl config elections stake-policy --fixed 50 -n node1

# Reset per-node override
nodectl config elections stake-policy --reset -n node1
```

---

## Security Recommendations

### Network Security

1. **Run Nodectl HTTP server on localhost only**

   ```json
   "http": {
     "bind": "127.0.0.1:8080"
   }
   ```

2. **Use SSH tunneling for remote access**

3. **Never expose the REST API to the public internet** — all endpoints are unauthenticated

### Key Security

1. **Use HashiCorp Vault for production** — provides audit logging, access control

2. **Non-extractable wallet keys** — store wallet keys as non-extractable to prevent export

3. **Separate access tokens** — use different Vault tokens for different services

### System Security

1. **Run as unprivileged user** — use a dedicated service user

2. **Restrict file permissions**

   ```bash
   chmod 600 /etc/nodectl/config.json
   ```

3. **Monitor logs** — set up log monitoring and alerts for suspicious activity

---

## Troubleshooting

### Debug Mode

Set log level to debug in your config file and restart service:

```json
{
  ...
  "log": {
    ...
    "level": "debug"
    ...
  }
  ...
}
```

Or override temporarily via environment variable:
```bash
RUST_LOG=debug nodectl service --config=nodectl-config.json
```
