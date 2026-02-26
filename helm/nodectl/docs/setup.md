# Validator setup guide

> **Alpha software.** nodectl is under active development. Configuration format, CLI interface, and Helm chart values may change between releases without notice.

Step-by-step guide for deploying nodectl and configuring it to manage TON validators.

## Table of contents

- [Prerequisites](#prerequisites)
- [Step 1: Deploy the chart](#step-1-deploy-the-chart)
- [Step 2: Configure](#step-2-configure)
- [Step 3: Set up keys](#step-3-set-up-keys)
- [Step 4: Restart the service](#step-4-restart-the-service)
- [Step 5: Fund and verify](#step-5-fund-and-verify)
- [Migrating an existing deployment](#migrating-an-existing-deployment)
- [Troubleshooting](#troubleshooting)

---

## Prerequisites

| Requirement | Description |
|-------------|-------------|
| **TON validator nodes** | One or more TON Rust validators with Control Server enabled (default port: 50000) |
| **Kubernetes cluster** | With Helm 3 installed |
| **TON HTTP API** | A TON Rust fullnode with JSON-RPC enabled (see [node-config.md](../../ton-rust-node/docs/node-config.md)), or a public endpoint like [toncenter.com](https://toncenter.com/api/v2/) |
| **Control Server keys** | Server public key (base64) and client private key (base64) for each node |

### Control Server

Each validator must have Control Server enabled. The [ton-rust-node](https://github.com/rsquad/ton-rust-node) Helm chart exposes it on port 50000 by default (`ports.control: 50000`).

You need two values per node:

- **Server public key** (base64) — identifies the node, used by nodectl to verify the connection
- **Client private key** (base64) — authenticates nodectl to the node's Control Server

If nodectl runs in a different cluster than the validators, the Control Server port must be externally reachable (e.g. via LoadBalancer).

---

## Step 1: Deploy the chart

### Create a vault secret

See [Secrets Vault](../../ton-rust-node/docs/vault.md) for vault setup, URL formats, and security details.

```bash
kubectl create secret generic nodectl-vault \
  --from-literal=VAULT_URL="file:///nodectl/data/vault.json&master_key=$(openssl rand -hex 32)"
```

### Install the chart

```bash
helm install my-nodectl oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl \
  --set vault.secretName=nodectl-vault
```

The init container automatically generates a default `config.json` on the PVC. The service starts but won't participate in elections until configured.

> **Tip:** Add `--set "securityContext.capabilities.add[0]=IPC_LOCK"` if you want file-based vault to use `mlock()` for memory protection. Not required — the service works without it.

### Exec into the pod

```bash
kubectl exec -it deploy/my-nodectl -- sh
```

All commands in Steps 2–3 run inside the pod. The `CONFIG_PATH` environment variable is set, so you don't need `-c` flags.

---

## Step 2: Configure

### Add nodes

Add each validator node with its Control Server endpoint, public key, and the vault secret name for the client key:

```bash
nodectl config node add \
  -n node0 \
  -e "10.0.0.1:50000" \
  -p "5RW/0ICaNBC6Qd0bWIHj3ujha2VUJETKEWXH6imIHWI=" \
  -s client-secret
```

| Flag | Description |
|------|-------------|
| `-n` | Unique node name |
| `-e` | Control Server endpoint (`IP:PORT`) |
| `-p` | Server public key (base64) |
| `-s` | Vault secret name for the client private key |

Repeat for each node. Multiple nodes can share the same client secret if they use the same client key.

### Set TON HTTP API

nodectl needs access to a TON HTTP API endpoint for blockchain queries (elector contract, config parameters, wallet balances). You can use:

- A TON Rust fullnode with JSON-RPC enabled (see [node-config.md](../../ton-rust-node/docs/node-config.md))
- A public endpoint such as `https://toncenter.com/api/v2/`

```bash
nodectl config ton-http-api set -u "http://<fullnode-ip>:8081"
```

### Add wallets

Create **one wallet per node**. Each wallet references a vault key:

```bash
nodectl config wallet add -n wallet0 -s wallet-key-0
nodectl config wallet add -n wallet1 -s wallet-key-1
nodectl config wallet add -n wallet2 -s wallet-key-2
```

| Flag | Description | Default |
|------|-------------|---------|
| `-n` | Wallet name | — |
| `-s` | Vault secret name for the wallet key | — |
| `-v` | Wallet contract version | `V3R2` |
| `-i` | Subwallet ID | `42` |
| `-w` | Workchain | `-1` |

> **Why one wallet per node?** The SNP contract address is computed from `owner_address + validator_wallet_address`. If two nodes share a wallet, they produce the same pool address and cannot participate in elections independently. See [Key concepts](../README.md#single-nominator-pool-snp).

### Add pools

Add Single Nominator Pools with the owner address. The pool contract address is computed automatically on startup:

```bash
nodectl config pool add -n pool0 --owner="<OWNER_ADDRESS>"
nodectl config pool add -n pool1 --owner="<OWNER_ADDRESS>"
nodectl config pool add -n pool2 --owner="<OWNER_ADDRESS>"
```

> **Note:** Use `--owner=` (with `=`) when the address starts with `-1:` — otherwise the CLI parser treats `-1:...` as a flag.

If the pool contract is already deployed, pass its address instead:

```bash
nodectl config pool add -n pool0 -a "-1:<POOL_CONTRACT_ADDRESS>"
```

### Add bindings

Bind each node to its wallet and pool:

```bash
nodectl config bind add -n node0 -w wallet0 -p pool0
nodectl config bind add -n node1 -w wallet1 -p pool1
nodectl config bind add -n node2 -w wallet2 -p pool2
```

### Set stake policy

```bash
# Split50 — stake half the available balance (default)
nodectl config stake-policy --split50

# Or fixed amount (in nanoTON):
nodectl config stake-policy --fixed 500000000000000

# Or minimum required:
nodectl config stake-policy --minimum

# Per-node override:
nodectl config stake-policy --fixed 100000000000000 --node node0
```

See [elections.md](elections.md) for details on stake policies.

---

## Step 3: Set up keys

### Import the control client key

The control client key authenticates nodectl to the validator's Control Server. It must be **extractable** — nodectl reads the raw private key to connect.

```bash
nodectl key import -n client-secret -k "<PRIVATE_KEY_BASE64>" --extractable
```

The corresponding public key must be listed in the validator node's `control_server.clients.list`.

If each node uses a different client key, import each one with a unique name matching the `-s` flag used in `config node add`.

### Generate wallet keys

Create a key in the vault for each wallet:

```bash
nodectl key add -n wallet-key-0
nodectl key add -n wallet-key-1
nodectl key add -n wallet-key-2
```

> **Important:** Wallet keys are **not** auto-generated. If a vault secret referenced by a wallet doesn't exist, the service fails on startup.

### Master wallet key

The master wallet key (`master-wallet-secret` by default) is **auto-generated** on first service start. You do not need to create it manually.

### Verify keys

```bash
nodectl key ls
```

---

## Step 4: Restart the service

Exit the pod shell and restart the pod to pick up the new configuration:

```bash
kubectl rollout restart deploy/my-nodectl
```

On startup, nodectl:

1. Opens the vault and loads all keys
2. Auto-generates the master wallet key (if not present)
3. Opens all wallets and pools via bindings
4. Starts `contracts_task` (auto-deploy)
5. Starts the elections task

---

## Step 5: Fund and verify

### Find the master wallet address

The master wallet address appears in the logs on first start:

```bash
kubectl logs deploy/my-nodectl | grep -i "master wallet"
```

### Fund the master wallet

The `contracts_task` deploys contracts from the master wallet. Fund it with:

- **1 TON per wallet** (deployment cost)
- **1 TON per pool** (deployment cost)
- **~2 TON reserve** (gas fees)

For 3 nodes: ~8 TON. For 9 nodes: ~20 TON.

### Wait for auto-deploy

Watch the logs for contract deployment progress:

```bash
kubectl logs deploy/my-nodectl -f
```

The `contracts_task` automatically:

1. Deploys the master wallet contract
2. Deploys each validator wallet (sends 1 TON from master)
3. Deploys each SNP pool contract (sends 1 TON from master)

Look for: `all contracts are ready` — all wallets and pools are deployed.

### Fund the pools

Each pool needs at least `min_stake` TON to participate in elections. Transfer funds to each pool address from the pool owner wallet.

Pool addresses are logged on startup.

### Enable elections

Nodes do not participate in elections by default. Enable each binding explicitly:

```bash
kubectl exec deploy/my-nodectl -- nodectl config elections enable node0 node1 node2
```

The service picks up the change automatically and starts participating in the next election round.

### Verify elections

Check the binding status:

```bash
kubectl exec deploy/my-nodectl -- nodectl config elections show
```

Expected log messages:

- `elections are open: id=...` — new election round detected
- `send stake` — stake sent to the Elector
- `no active elections` — waiting for next round

See [elections.md](elections.md) for binding statuses, stake policies, and election management.

---

## Migrating an existing deployment

nodectl configuration should only be managed through the CLI — do not edit `config.json` by hand. However, if you need to migrate nodectl to a different cluster or namespace, you can transfer the config and vault files from the existing PVC.

### 1. Copy config and vault from the old pod

```bash
kubectl cp deploy/old-nodectl:/nodectl/data/config.json ./config.json
kubectl cp deploy/old-nodectl:/nodectl/data/vault.json ./vault.json
```

### 2. Create K8s resources in the new location

```bash
# Seed config via a Secret
kubectl create secret generic nodectl-config \
  --from-file=config.json=./config.json

# Recreate the vault secret with the same master key as the original
kubectl create secret generic nodectl-vault \
  --from-literal=VAULT_URL="file:///nodectl/data/vault.json&master_key=<ORIGINAL_MASTER_KEY>"
```

### 3. Deploy with initConfig

```bash
helm install my-nodectl oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl \
  --set vault.secretName=nodectl-vault \
  --set initConfig.secretName=nodectl-config
```

The init container copies `config.json` from the Secret to the PVC **only on first run** (if the file doesn't exist yet). After that, the PVC config is the source of truth.

### 4. Restore the vault file

The vault file must be copied onto the PVC manually:

```bash
kubectl cp ./vault.json deploy/my-nodectl:/nodectl/data/vault.json
kubectl rollout restart deploy/my-nodectl
```

---

## Troubleshooting

### Service fails: "vault is not set"

`VAULT_URL` environment variable is not set. Check that `vault.secretName` or `vault.url` is configured in Helm values and the K8s Secret exists. See [vault.md](../../ton-rust-node/docs/vault.md).

### Secret not found in vault

Keys are not auto-generated (except the master wallet key). Create all referenced keys before starting the service:

```bash
kubectl exec -it deploy/my-nodectl -- sh
nodectl key add -n <secret-name>
kubectl rollout restart deploy/my-nodectl
```

> **Important:** nodectl does not auto-reload changes. After importing or generating keys via CLI, **restart the pod** for the service to pick them up.

### All pool addresses are identical

All nodes share the same wallet. The SNP address depends on the validator wallet address — use one wallet per node. See [Why one wallet per node?](#add-wallets).

### Probes failing

The default generated config uses `http.bind: "127.0.0.1:8080"`. Kubernetes probes need to reach the pod from outside localhost. Edit the config inside the pod:

Change `"bind": "127.0.0.1:8080"` to `"bind": "0.0.0.0:8080"`.

### Debug mode

Use `debug.sleep=true` to exec into the pod without starting the nodectl service. Useful for troubleshooting configuration or vault issues:

```bash
helm upgrade my-nodectl oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl \
  --set vault.secretName=nodectl-vault \
  --set debug.sleep=true
```

### Verbose logging

```yaml
logLevel: debug
```
