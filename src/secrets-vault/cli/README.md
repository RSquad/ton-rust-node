# Secrets-Vault CLI

A command-line interface for managing encrypted secrets in a vault.

## Usage

All commands require a vault URL:

```bash
secrets-vault-cli --url='<backend_url>' <command> [options]
```

## Supported Backends

### File Backend

```
file://<path_to_vault>?master_key=<64_character_hex_key>
```

- **path**: Location of the vault file (e.g., `vault.json`)
- **master_key**: 256-bit key in hexadecimal (64 characters)

### HashiCorp Vault Backend

```
hashicorp://<vault_address>?api_key=<token>&namespace=<namespace>
```

- **vault_address**: HashiCorp Vault server URL (e.g., `https://vault.example.com:8200`)
- **api_key**: Authentication token
- **namespace**: Vault namespace (optional)

## Commands

| Command    | Description                              |
|------------|------------------------------------------|
| `init`     | Create a new empty vault                 |
| `list`     | List all secrets in the vault            |
| `get`      | Get details of a specific secret         |
| `import`   | Add a new secret to the vault            |
| `generate` | Generate a new cryptographic key pair    |
| `delete`   | Delete one or more secrets               |
| `sign`     | Sign data using a stored key             |
| `verify`   | Verify a signature against stored key    |

## Examples

### Initialize a New Vault

```bash
secrets-vault-cli --url='file://vault.json?master_key=<KEY_HEX>' init
```

### Generate a Key Pair

```bash
# File backend
secrets-vault-cli --url='file://vault.json?master_key=<KEY_HEX>' generate \
  --secret-id secret_01 \
  --algorithm Ed25519
```

```bash
# HashiCorp Vault backend (extractable)
secrets-vault-cli --url='hashicorp://https://vault.example.com:8200?api_key=<API_KEY>&namespace=<NAMESPACE>' generate \
  --secret-id test_01 \
  --algorithm Ed25519 \
  --extractable
```

### Import a Secret

```bash
# Import an Ed25519 key pair
secrets-vault-cli --url='file://vault.json?master_key=<KEY_HEX>' import \
  --algorithm ed25519 \
  --secret-id secret_03 \
  --extractable \
  --data <DATA_HEX>
```

### Get Secret Details

```bash
secrets-vault-cli --url='hashicorp://https://vault.example.com:8200?api_key=<API_KEY>&namespace=<NAMESPACE>' get \
  --secret-id test_02
```

### List All Secrets

```bash
secrets-vault-cli --url='file://vault.json?master_key=<KEY_HEX>' list
```

### Sign Data

```bash
secrets-vault-cli --url='file://vault.json?master_key=<KEY_HEX>' sign \
  --secret-id secret_01 \
  --data <DATA_HEX>
```

### Verify a Signature

```bash
secrets-vault-cli --url='file://vault.json?master_key=<KEY_HEX>' verify \
  --secret-id secret_01 \
  --data <DATA_HEX> \
  --signature <SIGNATURE_HEX>
```

### Delete Secrets

```bash
secrets-vault-cli --url='file://vault.json?master_key=<KEY_HEX>' delete secret_01
```

## Command Reference

### `generate`

Generate a new cryptographic key pair.

| Option               | Required | Description                              |
|----------------------|----------|------------------------------------------|
| `--secret-id <ID>`   | Yes      | Unique identifier for the secret         |
| `--algorithm <ALG>`  | Yes      | Cryptographic algorithm (e.g., `Ed25519`)|
| `--extractable`      | No       | Allow secret to be exported              |

### `get`

Get details of a specific secret.

| Option               | Required | Description                              |
|----------------------|----------|------------------------------------------|
| `--secret-id <ID>`   | Yes      | ID of the secret to retrieve             |

### `sign`

Sign data using a stored key.

| Option               | Required | Description                              |
|----------------------|----------|------------------------------------------|
| `--secret-id <ID>`   | Yes      | ID of the signing key                    |
| `--data <HEX>`       | Yes      | Data to sign (hexadecimal)               |

### `verify`

Verify a signature.

| Option               | Required | Description                              |
|----------------------|----------|------------------------------------------|
| `--secret-id <ID>`   | Yes      | ID of the verification key               |
| `--data <HEX>`       | Yes      | Original data (hexadecimal)              |
| `--signature <HEX>`  | Yes      | Signature to verify (hexadecimal)        |

### `import`

Import an existing secret.

| Option               | Required | Description                              |
|----------------------|----------|------------------------------------------|
| `--secret-id <ID>`   | Yes      | Unique identifier for the secret         |
| `--data <HEX>`       | Yes      | Secret data (hexadecimal)                |
| `--algorithm <ALG>`  | No       | Algorithm type (default: `None`)         |
| `--extractable`      | No       | Allow secret to be exported              |

### `delete`

Delete one or more secrets.

```bash
secrets-vault-cli --url='<url>' delete <secret_id> [<secret_id>...]
```

## Secret Properties

| Property    | Description                                      |
|-------------|--------------------------------------------------|
| ID          | Unique identifier for the secret                 |
| Algorithm   | Cryptographic algorithm (`None`, `Ed25519`)      |
| Payload     | Data type stored (`Blob`, `KeyPair`)             |
| Extractable | Whether the secret can be exported (`Yes`/`No`)  |
| Created     | Timestamp when the secret was added              |
| Pub Key     | Public key (for key pairs)                       |

## Supported Algorithms

| Algorithm | Key Size | Payload Type | Operations   |
|-----------|----------|--------------|--------------|
| None      | Any      | Blob         | Store only   |
| Ed25519   | 32 bytes | KeyPair      | Sign, Verify |
