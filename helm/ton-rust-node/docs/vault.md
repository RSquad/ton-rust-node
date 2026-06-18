# Secrets Vault

The node and [nodectl](../../nodectl/docs/setup.md) share the same vault format and encryption. Both read the vault URL from the `VAULT_URL` environment variable.

When configured, private keys (ADNL, control server, liteserver) are stored in an encrypted vault file instead of plaintext in `config.json`.

> Without vault, private keys remain in `config.json` as plaintext. This is acceptable for fullnodes and liteservers. For validators, configuring a vault is recommended.

## Backends

| Backend | URL scheme | Where secrets live | Typical use |
|---------|------------|--------------------|-------------|
| **File** | `file://` | Encrypted JSON file on the node's PVC, AES-256-GCM under a master key | Single-cluster deployments, simplest setup |
| **HashiCorp Vault** | `hashicorp://` | Remote Vault — Ed25519 keys in Transit engine, blobs in KV v2 | Multi-tenant infra, shared key management, centralised audit |

For the full `VAULT_URL` grammar (every accepted query parameter, defaults, KV path layout) see the [secrets-vault README](../../../src/secrets-vault/README.md#vault-url-schemes).

## File backend

### 1. Create a Kubernetes Secret

```bash
kubectl create secret generic ton-node-vault \
  --from-literal=VAULT_URL="file:///keys/vault.json?master_key=$(openssl rand -hex 32)"
```

The master key is a 32-byte AES-256 encryption key (64 hex characters). Store it securely — anyone with the key can decrypt the vault file.

### 2. Reference in Helm values

```yaml
vault:
  secretName: ton-node-vault
```

The chart injects `VAULT_URL` into the node container from the Secret.

Alternatively, pass the URL directly (not recommended for production):

```yaml
vault:
  url: "file:///keys/vault.json?master_key=<64-char-hex>"
```

## HashiCorp Vault backend

Ed25519 keys are managed via Vault's **Transit** engine. Blobs and per-secret metadata are stored in a **KV v2** engine. Both the node and nodectl charts can use the same Vault server — they need different prefixes, policies, and roles (described below).

> **Already running on the file backend?** To move an existing node's keys into HashiCorp without re-generating them, follow [Copying Node Secrets from File Storage to HashiCorp Vault](../../../src/secrets-vault/cli/COPY_FILE_TO_HASHICORP.md). Prepare the target Vault per the steps below first, then run the migration.

### VAULT_URL format

```
hashicorp://<vault_address>?<auth>&<vault_config>
```

`<vault_address>` is `host[:port]`, optionally prefixed with `http://` or `https://`. If the scheme is omitted, `https://` is assumed.

**Authentication** — choose one:

| Parameter    | Required           | Description                                          |
|--------------|--------------------|------------------------------------------------------|
| `auth`       | No                 | `token` (default) or `k8s`                           |
| `api_key`    | If `auth=token`    | Static Vault token                                   |
| `role`       | If `auth=k8s`      | Vault role bound to the Pod ServiceAccount           |
| `auth_mount` | No                 | Kubernetes auth mount path (default `kubernetes`)    |
| `jwt_path`   | No                 | ServiceAccount JWT path (default `/var/run/secrets/kubernetes.io/serviceaccount/token`) |

**Vault configuration:**

| Parameter             | Default    | Description                                          |
|-----------------------|------------|------------------------------------------------------|
| `prefer_local_crypto` | `false`    | Cache extractable private keys locally to sign without round-tripping to Transit |
| `transit_mount`       | `transit`  | Mount path of the Transit secret engine              |
| `transit_prefix`      | —          | Prefix inside Transit — becomes part of every key name. **No `/` allowed** |
| `kv_mount`            | `secret`   | Mount path of the KV v2 secret engine                |
| `kv_prefix`           | —          | Prefix inside the KV mount. Slashes are allowed      |

### KV path layout

The backend stores two kinds of data side-by-side under `kv_prefix`. A Vault policy must cover **both** subtrees:

| Logical store   | KV data path                           | KV metadata path                          |
|-----------------|----------------------------------------|-------------------------------------------|
| Blobs           | `<kv_mount>/data/blobs/<kv_prefix>/*`  | `<kv_mount>/metadata/blobs/<kv_prefix>/*` |
| Per-secret meta | `<kv_mount>/data/meta/<kv_prefix>/*`   | `<kv_mount>/metadata/meta/<kv_prefix>/*`  |

### Preparing the Vault server

The steps below run against your Vault server with a token that has admin rights (typically `root` or an equivalent operator policy). They are **shared between the node and nodectl** — only the placeholders differ per client.

Placeholders used throughout:

| Placeholder         | Example value      | Source                                    |
|---------------------|--------------------|-------------------------------------------|
| `<TRANSIT_MOUNT>`   | `ton-transit`      | `transit_mount` in the client's URL       |
| `<TRANSIT_PREFIX>`  | `validator-0`      | `transit_prefix` in the client's URL      |
| `<KV_MOUNT>`        | `ton`              | `kv_mount` in the client's URL            |
| `<KV_PREFIX>`       | `mainnet/validator-0` | `kv_prefix` in the client's URL        |
| `<AUTH_MOUNT>`      | `kubernetes`       | `auth_mount` in the client's URL          |
| `<ROLE>`            | `validator-0`      | `role` in the client's URL                |
| `<SA>`              | `validator-0-sa`   | Pod ServiceAccount name                   |

#### 1. Enable the engines

Idempotent — skip if already enabled.

```bash
vault secrets enable -path=<TRANSIT_MOUNT> transit
vault secrets enable -path=<KV_MOUNT> -version=2 kv
```

#### 2. Create the client policy

One policy per client (one per validator, one for nodectl). Each policy is scoped to that client's `<TRANSIT_PREFIX>` and `<KV_PREFIX>` only — nothing more.

```hcl
# Transit: per-prefix key management + crypto
path "<TRANSIT_MOUNT>/keys/<TRANSIT_PREFIX>.*"               { capabilities = ["create", "read", "update"] }
path "<TRANSIT_MOUNT>/keys/"                                 { capabilities = ["list"] }
path "<TRANSIT_MOUNT>/sign/<TRANSIT_PREFIX>.*"               { capabilities = ["update"] }
path "<TRANSIT_MOUNT>/verify/<TRANSIT_PREFIX>.*"             { capabilities = ["update"] }
path "<TRANSIT_MOUNT>/export/signing-key/<TRANSIT_PREFIX>.*" { capabilities = ["read"] }
path "<TRANSIT_MOUNT>/wrapping_key"                          { capabilities = ["read"] }

# KV v2: blobs + per-secret metadata under the prefix
path "<KV_MOUNT>/data/blobs/<KV_PREFIX>/*"     { capabilities = ["create", "read", "update", "delete"] }
path "<KV_MOUNT>/data/meta/<KV_PREFIX>/*"      { capabilities = ["create", "read", "update", "delete"] }
path "<KV_MOUNT>/metadata/blobs/<KV_PREFIX>/*" { capabilities = ["read", "list", "delete"] }
path "<KV_MOUNT>/metadata/meta/<KV_PREFIX>/*"  { capabilities = ["read", "list", "delete"] }
```

Write it as:

```bash
vault policy write <policy-name> ./<policy-name>.hcl
```

#### 3. Kubernetes auth (skip for static token)

Enable the auth method (idempotent):

```bash
vault auth enable -path=<AUTH_MOUNT> kubernetes
```

If the Vault server runs **inside** the same cluster, it auto-discovers the Kubernetes API. If it runs **outside**, configure it explicitly:

```bash
vault write auth/<AUTH_MOUNT>/config \
  kubernetes_host="https://<k8s-api>:6443" \
  kubernetes_ca_cert=@/path/to/ca.crt
```

Create one role per client, bound to that client's Pod ServiceAccount and policy:

```bash
vault write auth/<AUTH_MOUNT>/role/<ROLE> \
  bound_service_account_names=<SA> \
  bound_service_account_namespaces=<your-namespace> \
  policies=<ROLE> \
  ttl=10m
```

> Per-client isolation requires **one role per client** (one SA per Pod, one policy per role). Sharing a role across multiple SAs means any of those Pods can use any of the policies attached to the role.

### Node deployment

Each validator is its own Helm release of this chart. The chart attaches a ServiceAccount to the Pod — by default named after the release (e.g. release `validator-0` → SA `validator-0-sa` when `serviceAccount.name` is set accordingly). Each validator should have its own Vault prefix, policy, and role.

#### Suggested per-validator values

For validator `i` (e.g. `i = 0`):

| Field            | Value             |
|------------------|-------------------|
| Helm release     | `validator-0`     |
| SA               | `validator-0-sa`  |
| Policy           | `validator-0`     |
| Role             | `validator-0`     |
| `transit_prefix` | `validator-0`     |
| `kv_prefix`      | `mainnet/validator-0` (or `dev/validator-0` per environment) |

Apply the [policy template](#2-create-the-client-policy) and [role template](#3-kubernetes-auth-skip-for-static-token) with these substitutions.

#### VAULT_URL

```
hashicorp://http://vault.vault.svc:8200?auth=k8s&role=validator-0&transit_mount=ton-transit&transit_prefix=validator-0&kv_mount=ton&kv_prefix=mainnet/validator-0
```

#### Create the K8s Secret

```bash
kubectl create secret generic ton-node-vault \
  --from-literal=VAULT_URL='hashicorp://http://vault.vault.svc:8200?auth=k8s&role=validator-0&transit_mount=ton-transit&transit_prefix=validator-0&kv_mount=ton&kv_prefix=mainnet/validator-0'
```

For a static token instead of Kubernetes auth, swap the URL:

```bash
kubectl create secret generic ton-node-vault \
  --from-literal=VAULT_URL='hashicorp://https://vault.example.com:8200?api_key=hvs.xxx&transit_mount=ton-transit&transit_prefix=validator-0&kv_mount=ton&kv_prefix=mainnet/validator-0'
```

#### Helm values

```yaml
vault:
  secretName: ton-node-vault

serviceAccount:
  enabled: true
  name: validator-0-sa     # must match bound_service_account_names in the role
```

## Values reference

| Parameter | Description | Default |
|-----------|-------------|---------|
| `vault.url` | Vault URL (plain text) | `""` |
| `vault.secretName` | Existing Secret containing the vault URL | `""` |
| `vault.secretKey` | Key inside the Secret | `"VAULT_URL"` |

When `vault.secretName` is set, it takes precedence over `vault.url`.

## Troubleshooting

**"vault is not set"** — `VAULT_URL` environment variable is not set. Check that `vault.secretName` or `vault.url` is configured in Helm values and the K8s Secret exists.

**Secret not found in vault** — Keys are not auto-generated (except the master wallet key in nodectl). Create all referenced keys before starting the service.

**`permission denied` on KV writes** — The policy is missing one of the two subtrees (`data/blobs/...` or `data/meta/...`). Both are required, see [KV path layout](#kv-path-layout).

**`permission denied` on Transit `sign`/`export`** — The policy paths must match the `transit_prefix` you put into the URL exactly, with `.*` to cover per-key suffixes (e.g. `validator-0.*`, not `validator-0/*`).

## Important

- Vault is configured **only** via the `VAULT_URL` environment variable. The `secrets_vault_config` field in `config.json` is no longer supported.
- The file-backend vault file is stored on the `keys` volume (`/keys`), which has `helm.sh/resource-policy: keep` by default — it survives `helm uninstall`.
- The node and nodectl can share the **same Vault server** but should use **different prefixes, policies, and roles**. They must not point at the same `transit_prefix`/`kv_prefix` — that would let either client clobber the other's keys.
