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
- [Setup REST API authentication](#setup-rest-api-authentication)
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

nodectl supports two vault backends. Pick one based on where you want the
secrets to live.

| Backend | URL scheme | Where secrets live | Typical use |
|---------|------------|--------------------|-------------|
| **File** | `file://` | Encrypted JSON file on the nodectl PVC, AES-256-GCM under a master key | Single-cluster deployments, simplest setup |
| **HashiCorp Vault** | `hashicorp://` | Remote Vault — Ed25519 keys in Transit engine, blobs in KV v2 | Multi-tenant infra, shared key management, centralised audit |

#### File backend

```bash
kubectl create secret generic nodectl-vault \
  --from-literal=VAULT_URL="file:///nodectl/data/vault.json?master_key=$(openssl rand -hex 32)"
```

The master key is a 32-byte AES-256 encryption key (64 hex characters). Store
it securely — anyone with the key can decrypt the vault file.

#### HashiCorp Vault backend

Prepare the Vault server (enable the Transit and KV v2 engines, create the policy and — for Kubernetes auth — the role) per [vault.md → HashiCorp Vault backend](../../ton-rust-node/docs/vault.md#hashicorp-vault-backend). The procedure is identical for nodectl and the node chart; only the placeholders differ.

Use these values when applying the policy and role templates:

| Placeholder        | nodectl value     |
|--------------------|-------------------|
| `<TRANSIT_MOUNT>`  | `ton-transit`     |
| `<TRANSIT_PREFIX>` | `nodectl`         |
| `<KV_MOUNT>`       | `ton`             |
| `<KV_PREFIX>`      | `nodectl`         |
| `<AUTH_MOUNT>`     | `kubernetes`      |
| `<ROLE>`           | `nodectl`         |
| `<SA>`             | `nodectl-sa`      |

For the full `VAULT_URL` grammar (every accepted query parameter, defaults) see [secrets-vault README](../../../src/secrets-vault/README.md#vault-url-schemes).

##### Create the K8s Secret

Pick one of the two URLs and put it into a `Secret` referenced by `vault.secretName`.

**Static token** — for development or out-of-cluster Vault:

```bash
kubectl create secret generic nodectl-vault \
  --from-literal=VAULT_URL='hashicorp://https://vault.example.com:8200?api_key=hvs.xxx&transit_mount=ton-transit&transit_prefix=nodectl&kv_mount=ton&kv_prefix=nodectl'
```

**Kubernetes auth** — recommended for in-cluster Vault:

```bash
kubectl create secret generic nodectl-vault \
  --from-literal=VAULT_URL='hashicorp://http://vault.vault.svc:8200?auth=k8s&auth_mount=kubernetes&role=nodectl&transit_mount=ton-transit&transit_prefix=nodectl&kv_mount=ton&kv_prefix=nodectl'
```

##### Helm values

For Kubernetes auth, the chart must attach the SA bound to the Vault role:

```yaml
vault:
  secretName: nodectl-vault

serviceAccount:
  enabled: true        # chart creates the SA
  name: nodectl-sa     # must match bound_service_account_names in the Vault role
  # OR, to attach an existing SA you manage yourself:
  # enabled: false
  # name: my-existing-sa
```

##### Migrating from the file backend

If you already run nodectl on the file backend and want to move secrets into HashiCorp Vault without re-generating keys, use the dedicated migration command — see [copy-file-to-hashicorp.md](copy-file-to-hashicorp.md). The target Vault must already be prepared per the steps above.

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

Set the primary endpoint:

```bash
nodectl config ton-http-api set -u "http://<fullnode-ip>:8081"
```

Optionally add failover endpoints. nodectl tries them in order if the primary is unreachable:

```bash
nodectl config ton-http-api add -u "http://<backup-ip>:8081"
```

Each endpoint can have its own API key:

```bash
nodectl config ton-http-api add -u "https://toncenter.com/api/v2/" -k "<API_KEY>"
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
| `-v` | Wallet contract version (`V1R3`, `V3R2`, `V4R2`, `V5R1`) | `V3R2` |
| `-i` | Subwallet ID | `42` |
| `-w` | Workchain | `-1` |

> **Why one wallet per node?** The SNP contract address is computed from `owner_address + validator_wallet_address`. If two nodes share a wallet, they produce the same pool address and cannot participate in elections independently. See [Key concepts](../README.md#single-nominator-pool-snp). TONCore pools use a different model — see [TONCore nominator pools](#toncore-nominator-pools).

### Add pools

nodectl supports two nominator pool types. Choose based on your operational model:

| | **SNP** (Single Nominator Pool) | **TONCore** (Nominator Pool) |
|---|---|---|
| **Pool contracts** | 1 per node | 2 per node (even + odd slots) |
| **Round participation** | Every round from one pool | Each pool participates in **1 of 2** consecutive rounds |
| **Validator stake deposit** | Not required — pool balance is the stake | Required — validator must deposit into each pool via `deposit-validator` |
| **Stake policies** | All supported (Minimum, Fixed, Split50, AdaptiveSplit50) | Split50 / AdaptiveSplit50 behave differently — stake the **full liquid balance** instead of half (see [below](#stake-policy-limitations)) |

#### Single Nominator Pool (SNP)

Add Single Nominator Pools with the owner address. The pool contract address is computed automatically on startup:

```bash
nodectl config pool add -n pool0 --owner=”<OWNER_ADDRESS>”
nodectl config pool add -n pool1 --owner=”<OWNER_ADDRESS>”
nodectl config pool add -n pool2 --owner=”<OWNER_ADDRESS>”
```

> **Note:** Use `--owner=` (with `=`) when the address starts with `-1:` — otherwise the CLI parser treats `-1:...` as a flag.

If the pool contract is already deployed, pass its address instead:

```bash
nodectl config pool add -n pool0 -a “-1:<POOL_CONTRACT_ADDRESS>”
```

#### Nominator pool (TONCore)

##### How it works

A Nominator Pool participates in **only one of every two consecutive election rounds**. The TON Elector alternates between even and odd rounds, and each on-chain pool contract is bound to one of them. This means:

- With a **single slot** (even **or** odd) you validate in **every other round** — skipping the alternate one.
- To validate in **every round**, configure **both slots** (`--even` and `--odd`) under the same pool name. nodectl automatically picks the correct slot for each round.

Unlike SNP — where the pool balance is the stake — a TONCore pool requires a **validator stake deposit**. The validator must explicitly send funds into each pool contract via `deposit-validator` before elections can proceed. See [Validator stake deposit](#validator-stake-deposit) below.

##### Adding pools

Both slots are registered under a single pool name (e.g. `core0`) with two separate commands. You bind the node once to that pool name; nodectl manages both slot addresses.

**If the pool contracts are already deployed**, pass their addresses:

```bash
nodectl config pool add core -n core0 --even --address=”-1:<EVEN_POOL_ADDRESS>”
nodectl config pool add core -n core0 --odd  --address=”-1:<ODD_POOL_ADDRESS>”
```

**Otherwise**, register slots with deploy parameters (adjust shares and minima to your network and economics):

```bash
nodectl config pool add core -n core0 --even \
  --validator-share 1000 \
  --min-validator-stake 10000 \
  --min-nominator-stake 10000

nodectl config pool add core -n core0 --odd \
  --validator-share 1000 \
  --min-validator-stake 10001 \
  --min-nominator-stake 10000
```

> **Distinct slot addresses:** the **even** and **odd** contracts must have **different** on-chain addresses. The derived address is a function of the validator wallet plus the slot’s **init parameters**; identical params on both slots yield the **same** address. A common pattern is to change **`min-validator-stake` by one unit** (TON) between slots (this example uses **`10000` vs `10001`**). You can vary any numeric deploy field as long as the resulting init data differs.

| Flag | Meaning |
|------|---------|
| `--even` / `--odd` | Which validation-round slot this contract covers (required; pick exactly one per command). |
| `--validator-share` | Validator reward share in **basis points** (e.g. `1000` = 10%). |
| `--min-validator-stake` | Minimum validator stake locked in the pool, **TON**. If omitted, the service default is **`100_000` TON**. |
| `--min-nominator-stake` | Minimum stake per nominator, **TON**. If omitted, the service default is **`10_000` TON**. |

##### Validator stake deposit

TONCore pools require a validator stake — funds the validator locks into the pool before it can participate in elections. This is separate from nominator deposits. Each `deposit-validator` call sends TON from the **validator wallet** (resolved via the binding) into the specified pool slot.

The deposit amount must be **>=** the slot’s `min-validator-stake`. Fund the validator wallet on-chain first; it must cover the deposit amount plus gas.

After the binding exists and the validator wallet is funded, run:

```bash
nodectl config pool deposit-validator -b node0 -a 10000 --pool-even
nodectl config pool deposit-validator -b node0 -a 10001 --pool-odd
```

| Flag | Description |
|------|-------------|
| `-b` | Binding name (same as the node name in `config bind add -n …`). Resolves the **validator wallet** and pool. |
| `-a` | **Validator stake** in **TON**, sent from the binding’s wallet into the pool slot. |
| `--pool-even` / `--pool-odd` | Which TONCore slot receives the deposit (default if omitted: even). |

> **`deposit-validator` is executed by the CLI with local vault + RPC** (not via the REST API yet). Run it from the pod shell where `CONFIG_PATH` and `VAULT_URL` are set.

##### Stake policy limitations

**Split50** and **AdaptiveSplit50** are designed for SNP, where a single pool re-splits its balance across alternating rounds. TONCore uses **two separate pools** that each stake in only one round, so splitting inside a round has no effect. When the binding points at a TONCore pool, nodectl ignores these two policies and stakes the **full liquid balance** of the active slot’s pool (still floored at `min_stake`).

To control per-round exposure with TONCore, use **Fixed** or **Minimum** instead. See [staking strategies](elections.md) for details.

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

### Configure logging

The generated config includes a `log` section with these defaults:

```json
{
  "log": {
    "level": "info",
    "output": "console",
    "rotation": "daily",
    "max_size_mb": 50,
    "max_files": 10
  }
}
```

| Field | Default | Description |
|-------|---------|-------------|
| `level` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `output` | `console` | Where to write: `console`, `file`, or `all` (both) |
| `path` | — | Log file path (required if `output` is `file` or `all`) |
| `max_size_mb` | `50` | Max size of a single log file in MB before rotation |
| `max_files` | `10` | Number of rotated log files to keep |
| `rotation` | `daily` | Rotation schedule: `daily`, `hourly`, or `never` |

View current settings:

```bash
nodectl config log ls
```

Update one or more settings with `nodectl config log set`:

```bash
# Set log level
nodectl config log set --level debug

# Enable file logging (path is required for 'file' and 'all' output modes)
nodectl config log set --output all --path /var/log/nodectl/service.log

# Adjust rotation policy and limits
nodectl config log set --rotation hourly --max-size-mb 100 --max-files 20
```

| Option | Short | Description |
|--------|-------|-------------|
| `--level` | `-l` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `--output` | `-o` | Output mode: `console`, `file`, or `all` |
| `--path` | `-p` | Log file path |
| `--rotation` | `-r` | Rotation policy: `daily`, `hourly`, or `never` |
| `--max-size-mb` | `-s` | Max log file size in MB |
| `--max-files` | `-f` | Max number of rotated log files to keep |

> **Note:** At least one option must be specified. If `--output` is set to `file` or `all`, a log file path must be configured via `--path` (either in the same command or previously).

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
3. Deploys each configured pool contract — SNP pools and TONCore even/odd slots (sends 1 TON from master per pool deployment)

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

## Setup REST API authentication

> **Authentication is enabled by default.** A freshly deployed nodectl has the `http.auth` section in its config with an empty user list — all protected endpoints return `401` until at least one user is created. The API is safe to expose only after you create a user and verify authentication works. See [nodectl-security.md](../../../src/node-control/docs/nodectl-security.md) for the full security model.

### 1. Create a user inside the pod

```bash
kubectl exec -it deploy/my-nodectl -- nodectl auth add -u <username> -r operator
```

The service picks up the new user automatically — no restart required.

### 2. Verify auth is working

```bash
kubectl exec deploy/my-nodectl -- nodectl api elections
```

Without a token, the command returns `401 Unauthorized`. This confirms the API is protected and safe to expose externally.

### 3. Log in and use the API

All `nodectl api` commands resolve the service URL in this order:

1. Explicit `--url` flag (e.g. `--url http://10.0.0.5:8080`)
2. `http.bind` value from `--config` (or `CONFIG_PATH`)

When running **inside the pod**, the config file is available and the URL is resolved automatically. When running **outside the pod** (e.g. from your workstation), pass `--url` explicitly — otherwise the command tries to read the local config and fails:

```bash
# Inside the pod — URL from config
kubectl exec -it deploy/my-nodectl -- nodectl api login <username>

# Outside the pod — explicit URL required
nodectl api login <username> --url http://<SERVICE_IP>:8080
```

```bash
# Get a token (inside the pod)
kubectl exec -it deploy/my-nodectl -- nodectl api login <username>
export NODECTL_API_TOKEN="<jwt>"

# Use the API (from outside, with --url)
nodectl api elections --url http://<SERVICE_IP>:8080

# Or set both token and URL, then run commands without flags
export NODECTL_API_TOKEN="<jwt>"
nodectl api elections --url http://<SERVICE_IP>:8080
```

### 4. Expose the Service REST API externally (Optional)

The chart creates a Kubernetes Service with configurable `service.type`. Set it to `NodePort` or `LoadBalancer` in your values, or keep the default `ClusterIP` and attach your own Ingress or reverse proxy to the Service by name. See `values.yaml` for all available `service.*` parameters.

The chart does not terminate TLS — the pod serves plain HTTP on port 8080. TLS should be handled by your load balancer, Ingress controller, or reverse proxy.

> **Warning:** Without TLS, passwords sent to `/auth/login` and JWT tokens in `Authorization` headers are transmitted in plain text. Always terminate TLS before the traffic leaves your trusted network — at the Ingress controller, load balancer, or reverse proxy.

> **Rate limiter:** Make sure your reverse proxy forwards the real client IP (e.g. `X-Forwarded-For`). Without it, the login rate limiter keys all requests to the proxy IP instead of the real client.

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
  --from-literal=VAULT_URL="file:///nodectl/data/vault.json?master_key=<ORIGINAL_MASTER_KEY>"
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

The default `http.bind` is `0.0.0.0:8080`, so probes should work out of the box. If you have overridden it to `127.0.0.1:8080`, change it back to `0.0.0.0:8080` — Kubernetes probes need to reach the pod from outside localhost.

### Debug mode

Use `debug.sleep=true` to exec into the pod without starting the nodectl service. Useful for troubleshooting configuration or vault issues:

```bash
helm upgrade my-nodectl oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl \
  --set vault.secretName=nodectl-vault \
  --set debug.sleep=true
```

### Logging

nodectl uses its built-in defaults when started by this chart. Use `kubectl logs` for runtime diagnostics. To adjust log level or enable file logging, see [Configure logging](#configure-logging).
