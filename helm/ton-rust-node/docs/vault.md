# Secrets Vault

The node and [nodectl](../../nodectl/docs/setup.md) share the same vault format and encryption. Both read the vault URL from the `VAULT_URL` environment variable.

When configured, private keys (ADNL, control server, liteserver) are stored in an encrypted vault file instead of plaintext in `config.json`.

> Without vault, private keys remain in `config.json` as plaintext. This is acceptable for fullnodes and liteservers. For validators, configuring a vault is recommended.

## Setup

### 1. Create a Kubernetes Secret

```bash
kubectl create secret generic ton-node-vault \
  --from-literal=VAULT_URL="file:///keys/vault.json&master_key=$(openssl rand -hex 32)"
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
  url: "file:///keys/vault.json&master_key=<64-char-hex>"
```

## Values reference

| Parameter | Description | Default |
|-----------|-------------|---------|
| `vault.url` | Vault URL (plain text) | `""` |
| `vault.secretName` | Existing Secret containing the vault URL | `""` |
| `vault.secretKey` | Key inside the Secret | `"VAULT_URL"` |

When `vault.secretName` is set, it takes precedence over `vault.url`.

## Vault URL formats

| Backend | URL format |
|---------|------------|
| File | `file:///keys/vault.json&master_key=<64-char-hex>` |

## Troubleshooting

**"vault is not set"** — `VAULT_URL` environment variable is not set. Check that `vault.secretName` or `vault.url` is configured in Helm values and the K8s Secret exists.

**Secret not found in vault** — Keys are not auto-generated (except the master wallet key in nodectl). Create all referenced keys before starting the service.

## Important

- Vault is configured **only** via the `VAULT_URL` environment variable. The `secrets_vault_config` field in `config.json` is no longer supported.
- The vault file is stored on the `keys` volume (`/keys`), which has `helm.sh/resource-policy: keep` by default — it survives `helm uninstall`.
- Both the node and nodectl must point to the **same vault** for key management to work correctly.
