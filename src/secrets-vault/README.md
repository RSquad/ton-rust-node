# Secrets Vault

A Rust library for secure secret management with pluggable storage backends and cryptographic implementations. Provides encrypted storage for key pairs, symmetric keys, and arbitrary blobs with OS-level memory protection.

## Features

- **Pluggable storage backends** — encrypted local JSON files or remote HashiCorp Vault
- **Pluggable cryptographic implementations** — `ed25519-dalek` (default) or `ton_block`- compatible implementation
- **Protected memory** — page-aligned, mlock'd, mprotect'd buffers with automatic zeroing on drop
- **Secret types** — Ed25519 key pairs, AES-256-GCM symmetric keys, arbitrary binary blobs
- **Async API** — fully async with `tokio`

## Quick Start

### As a Library

Add to your `Cargo.toml`:

```toml
[dependencies]
secrets-vault = { path = "../secrets-vault", features = ["file-storage-json", "crypto-default"] }
```

Create a vault from a URL:

```rust
use secrets_vault::{
    vault_builder::SecretVaultBuilder,
    types::{algorithm::Algorithm, secret_spec::SecretSpec},
};

let vault = SecretVaultBuilder::from_url("file:///path/to/vault.json?master_key=abcdef...64hex", DefaultCryptoFactory {}.new_crypto()?).await?;

// Generate an Ed25519 key pair
let spec = SecretSpec::new(Algorithm::Ed25519).extractable(true);
let secret = vault.generate_secret(&spec, &"my_key".into()).await?;
vault.flush().await?;

// Sign and verify
let keypair = vault.load(&"my_key".into()).await?;
let sig = keypair.as_keypair()?.sign(b"hello").await?;
keypair.as_keypair()?.verify(b"hello", &sig).await?;
```

### From Environment

Set `VAULT_URL` and open the vault:

```rust
let vault = SecretVaultBuilder::from_env(DefaultCryptoFactory {}.new_crypto()?).await?;
```

## Vault URL Schemes

### File Backend (`file://`)

```
file://<path>?master_key=<64_hex_chars>[&auto_migrate=true]
```

| Parameter      | Required | Description                                       |
|----------------|----------|---------------------------------------------------|
| `master_key`   | Yes      | 256-bit AES master key (64 hex characters)        |
| `auto_migrate` | No       | Auto-migrate storage format on open (default: true)|

Secrets are encrypted with AES-256-GCM under the master key and stored in a hierarchical JSON tree.

### HashiCorp Vault Backend (`hashicorp://`)

```
hashicorp://<vault_address>?api_key=<token>[&namespace=<ns>][&prefer_local_crypto=false]
```

**Authentication** — choose one method:

| Parameter   | Required | Description                                          |
|-------------|----------|------------------------------------------------------|
| `api_key`   | Yes*     | Static Vault token (`auth=token` or omit `auth`)     |
| `auth`      | No       | Auth method: `token` (default) or `k8s`              |
| `role`      | Yes**    | Vault role name (required when `auth=k8s`)           |
| `auth_mount`| No       | Kubernetes auth mount path (default: `kubernetes`)   |
| `jwt_path`  | No       | Path to service account JWT (default: `/var/run/secrets/kubernetes.io/serviceaccount/token`) |

\* Required when `auth=token` or `auth` is omitted. \*\* Required when `auth=k8s`.

**Vault configuration:**

| Parameter             | Default    | Description                                          |
|-----------------------|------------|------------------------------------------------------|
| `namespace`           | —          | Vault namespace                                      |
| `prefer_local_crypto` | `false`    | Cache extractable private keys locally               |
| `transit_mount`       | `transit`  | Mount path for Transit secret engine                 |
| `transit_prefix`      | —          | Path prefix within the transit mount (e.g. `mainnet` or `mainnet.validator-0`). No `/` or other URL-specific characters allowed. |
| `kv_mount`            | `secret`   | Mount path for KV v2 secret engine                   |
| `kv_prefix`           | —          | Path prefix within KV mount (e.g. `mainnet` or `mainnet/validator-0`) |

Ed25519 keys are managed via Transit engine. Blobs are stored in KV v2 engine.

**Examples:**

```bash
# Static token (default mounts)
hashicorp://vault:8200?api_key=hvs.xxx

# Kubernetes auth
hashicorp://vault:8200?auth=k8s&role=validator-0

# Custom mount paths + environment prefix
hashicorp://vault:8200?api_key=hvs.xxx&transit_mount=ton-transit&transit_prefix=mainnet.validator_0&kv_mount=ton&kv_prefix=mainnet.validator_0
```

## Core API

### SecretVault

| Method                              | Description                          |
|-------------------------------------|--------------------------------------|
| `generate_secret(spec, secret_id)`  | Generate and store a new secret      |
| `get(secret_id)`                    | Load a secret by ID                  |
| `put(secret, mode)`                 | Store a secret with mode control     |
| `delete(secret_id)`                 | Remove a secret                      |
| `exists(secret_id)`                 | Check if a secret exists             |
| `load_metadata(secret_id)`          | Get metadata without loading secret  |
| `list_metadata()`                   | List all secret metadata             |
| `flush()`                           | Persist pending changes to storage   |

### SecretSpec

Defines parameters for secret generation:

```rust
let spec = SecretSpec::new(Algorithm::Ed25519)
    .extractable(true)
    .with_tag("env", "production")
    .with_expiration(expires_at);

// For blobs with custom size
let blob_spec = SecretSpec::new(Algorithm::None).size(64);
```

### StoreMode

Controls `put()` behavior:

| Mode              | Behavior                          |
|-------------------|-----------------------------------|
| `NewOnly`         | Fail if secret already exists     |
| `ReplaceExists`   | Fail if secret does not exist     |
| `CreateOrReplace` | Always write                      |

### Secret Types

| Type           | Algorithm     | Operations            |
|----------------|---------------|-----------------------|
| `KeyPair`      | `Ed25519`     | sign, verify, export  |
| `SymmetricKey` | `Aes256Gcm`   | encrypt, decrypt      |
| `Blob`         | `None`        | read/write raw data   |

Access typed data via `secret.as_keypair()`, `secret.as_symmetric_key()`, or `secret.as_blob()`.

### Hierarchical Secret IDs

Secret IDs support `.`-delimited hierarchical paths:

```rust
use secrets_vault::make_secret_id;

let id = make_secret_id!("keys", "validators", "node_01");
// Stored at keys.validators.node_01 in the tree
```

## Architecture

```
SecretVault
  |
  +-- Storage (trait)
  |     +-- FileJsonStorage   [feature: file-storage-json]
  |     +-- HashicorpStorage  [feature: hashicorp-storage]
  |
  +-- CryptoFactory (trait)
  |     +-- DefaultCryptoFactory (ed25519-dalek)
  |
  +-- CryptoImpl<B: Ed25519Backend>
  |     +-- DefaultEd25519  [feature: crypto-default]
  |
  +-- ProtectedMemory
        (mlock + mprotect + zeroize-on-drop)
```

## Cargo Features

| Feature              | Description                                | Default |
|----------------------|--------------------------------------------|---------|
| `file-storage-json`  | Local encrypted JSON file storage          | Yes     |
| `crypto-default`     | Ed25519 via `ed25519-dalek` + AES-GCM      | Yes     |
| `hashicorp-storage`  | HashiCorp Vault remote backend             | No      |
| `secrets-vault-cli`  | CLI binary                                 | No      |

## Error Codes

Errors are categorized by numeric code ranges:

| Range   | Category         | Examples                                     |
|---------|------------------|----------------------------------------------|
| 1xx     | Secret errors    | not found, already exists, non-extractable   |
| 2xx     | Crypto errors    | invalid signature, decryption failed         |
| 3xx     | Storage errors   | corrupted, read/write failure, lock timeout  |
| 4xx     | Backend errors   | connection failed, auth failed               |
| 5xx     | Config errors    | invalid URL, missing master key              |
| 6xx     | Internal errors  | serialization, deserialization               |

All errors implement `std::error::Error` with `.code()` for the numeric code and `.message()` for context.

## CLI

See [cli/README.md](cli/README.md) for the command-line interface documentation.

Build with:

```bash
cargo build -p secrets-vault --features secrets-vault-cli
```
