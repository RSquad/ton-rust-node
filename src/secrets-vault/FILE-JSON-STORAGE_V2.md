# File-JSON Storage Format Specification

**Version:** 2
**Status:** Current
**Crate:** `secrets-vault`

## Overview

File-JSON Storage is a hierarchical, encrypted secret storage format that persists secrets as a JSON file on disk. Each secret's data and metadata are encrypted with AES-256-GCM using a master key, then hex-encoded and organized into a tree structure keyed by dot-separated secret IDs.

---

## 1. JSON File Structure

The vault file is a JSON object with two top-level fields:

```json
{
  "version": 2,
  "tree": { ... }
}
```

| Field     | Type   | Description                          |
|-----------|--------|--------------------------------------|
| `version` | `u32`  | Format version. Current value: **2** |
| `tree`    | object | Root `SecretNode` (see below)        |

### 1.1 SecretNode

Each node in the tree is a recursive structure:

```json
{
  "secret": { "encrypted_data": "<hex string>" },
  "children": {
    "<key>": { <SecretNode> },
    ...
  }
}
```

| Field       | Type                          | Presence                  | Description                            |
|-------------|-------------------------------|---------------------------|----------------------------------------|
| `secret`    | `StoredSecret` or absent      | Omitted if `null`         | Encrypted secret at this node          |
| `children`  | `Map<String, SecretNode>`     | Omitted if empty          | Child nodes keyed by path segment      |

### 1.2 StoredSecret

```json
{
  "encrypted_data": "a1b2c3d4..."
}
```

| Field            | Type   | Description                                         |
|------------------|--------|-----------------------------------------------------|
| `encrypted_data` | string | Hex-encoded byte sequence (nonce + ciphertext + tag) |

---

## 2. Secret ID and Path Mapping

Secret IDs are dot-separated hierarchical paths. Each unescaped `.` splits the ID into tree path segments.

### 2.1 Path Parsing Rules

| Sequence | Meaning              |
|----------|----------------------|
| `.`      | Path separator       |
| `\.`     | Literal dot          |
| `\\`     | Literal backslash    |

**Examples:**

| Secret ID                        | Path segments                       |
|----------------------------------|-------------------------------------|
| `keys.validators.node_01`       | `["keys", "validators", "node_01"]` |
| `data\.with\.dots.section`      | `["data.with.dots", "section"]`     |
| `simple`                         | `["simple"]`                        |

### 2.2 Tree Mapping

Given secret ID `keys.validators.node_01`, the JSON tree is:

```json
{
  "children": {
    "keys": {
      "children": {
        "validators": {
          "children": {
            "node_01": {
              "secret": { "encrypted_data": "..." }
            }
          }
        }
      }
    }
  }
}
```

Empty intermediate nodes (no secret, no children) are pruned on deletion.

### 2.3 Legacy Path Format (pre-v0.2.0)

Legacy IDs used `/` as separator and `\/` as escape. The system auto-detects legacy format by finding the first unescaped separator character and converts to the current `.`-separated format on read.

---

## 3. Encryption

All secrets are encrypted with **AES-256-GCM** using the vault master key.

### 3.1 Master Key

- **Source:** Environment variable containing a **64-character hexadecimal** string
- **Decoded size:** 32 bytes (256 bits)

### 3.2 Plaintext Layout (before encryption)

Before encryption, secret data and metadata are combined into a single binary buffer:

```
+------------------+------------------+----------------+-------------------+
| data_len (4B)    | meta_len (4B)    | data (N bytes) | metadata (M bytes)|
+------------------+------------------+----------------+-------------------+
```

| Offset   | Size    | Type      | Description                                  |
|----------|---------|-----------|----------------------------------------------|
| 0        | 4       | `u32 LE`  | Length of secret data in bytes (`N`)          |
| 4        | 4       | `u32 LE`  | Length of JSON metadata in bytes (`M`)        |
| 8        | N       | bytes     | Secret data (serialized `KeyMaterial`)        |
| 8 + N    | M       | bytes     | JSON-encoded `Metadata` object                |

**Invariant:** Total plaintext size = `8 + data_len + meta_len`

### 3.3 AES-256-GCM Encryption

The plaintext buffer is encrypted using AES-256-GCM:

1. Generate a **12-byte random nonce** (cryptographically secure PRNG)
2. Encrypt plaintext with AES-256-GCM using master key and nonce
3. Produces ciphertext + 16-byte authentication tag

### 3.4 Ciphertext Layout (stored in `encrypted_data`)

```
+------------------+--------------------+----------------+
| nonce (12 bytes) | ciphertext (C bytes) | tag (16 bytes) |
+------------------+--------------------+----------------+
```

| Offset   | Size    | Description                               |
|----------|---------|-------------------------------------------|
| 0        | 12      | AES-GCM nonce (random, unique per secret) |
| 12       | C       | Encrypted data                            |
| 12 + C   | 16      | GCM authentication tag                    |

**Minimum ciphertext size:** 28 bytes (12 nonce + 0 data + 16 tag)

This byte sequence is then **hex-encoded** to produce the `encrypted_data` JSON string value. The hex-encoded string length is therefore `2 * (12 + C + 16)` characters.

---

## 4. Decryption

### 4.1 Ciphertext Decryption

1. Hex-decode the `encrypted_data` string into bytes
2. Validate minimum length >= 28 bytes
3. Split: `nonce = bytes[0..12]`, `encrypted_payload = bytes[12..]`
4. Decrypt `encrypted_payload` using AES-256-GCM with the master key and nonce
5. GCM tag is verified automatically; failure indicates corruption or wrong key

### 4.2 Plaintext Parsing

1. Validate decrypted buffer length >= 8 bytes
2. Read `data_len = u32 LE` from bytes `[0..4]`
3. Read `meta_len = u32 LE` from bytes `[4..8]`
4. Validate: `decrypted_len == 8 + data_len + meta_len`
5. Extract `data = bytes[8 .. 8+data_len]`
6. Extract `metadata_json = bytes[8+data_len .. 8+data_len+meta_len]`
7. Deserialize `metadata_json` as `Metadata` (JSON)

---

## 5. Secret Data Serialization (KeyMaterial)

The `data` portion of the plaintext contains a serialized `KeyMaterial`:

```
+------------------------+--------------------------+---------------------+---------------------+
| secret_key_len (4B)    | public_key_len (4B)      | secret_key (S bytes)| public_key (P bytes)|
+------------------------+--------------------------+---------------------+---------------------+
```

| Offset   | Size | Type     | Description                              |
|----------|------|----------|------------------------------------------|
| 0        | 4    | `u32 LE` | Length of secret key (`S`); 0 if absent  |
| 4        | 4    | `u32 LE` | Length of public key (`P`); 0 if absent  |
| 8        | S    | bytes    | Secret (private) key bytes               |
| 8 + S    | P    | bytes    | Public key bytes                         |

**Invariant:** Total size = `8 + secret_key_len + public_key_len`. At least one of `S` or `P` must be > 0.

### 5.1 Algorithm-Specific Key Sizes

| Algorithm    | Payload Type    | secret_key         | public_key         |
|--------------|-----------------|--------------------|--------------------|
| `None`       | Blob            | Variable length    | Absent (`len = 0`) |
| `Aes256Gcm`  | SymmetricKey    | 32 bytes           | Absent (`len = 0`) |
| `Ed25519`    | KeyPair         | 32 bytes (seed)    | 32 bytes            |

---

## 6. Metadata

The metadata is serialized as JSON within the encrypted plaintext. Structure:

```json
{
  "secret_id": "keys.validators.node_01",
  "algorithm": "Ed25519",
  "extractable": true,
  "created_at": "2025-01-15T10:30:00Z",
  "expires_at": "2026-01-15T10:30:00Z",
  "tags": {
    "role": "validator",
    "network": "mainnet"
  }
}
```

| Field         | Type                          | Required | Default        | Description                                    |
|---------------|-------------------------------|----------|----------------|------------------------------------------------|
| `secret_id`   | `string` or `null`           | No       | `null`         | Dot-separated hierarchical identifier          |
| `algorithm`   | `string` enum                | Yes      | `"None"`       | One of: `"None"`, `"Aes256Gcm"`, `"Ed25519"`  |
| `extractable` | `bool`                       | Yes      | `false`        | Whether key material can be exported           |
| `created_at`  | `string` (ISO 8601 UTC)      | Yes      | current time   | Creation timestamp                             |
| `expires_at`  | `string` (ISO 8601 UTC)      | No       | omitted        | Expiration timestamp (omitted if none)         |
| `tags`        | `Map<string, string>`        | No       | omitted        | User-defined key-value pairs (omitted if empty)|

---

## 7. Supported Algorithms

| Algorithm     | Key Bits | Capabilities           | Payload Type    |
|---------------|----------|------------------------|-----------------|
| `None`        | 0        | Raw blob storage       | `Blob`          |
| `Aes256Gcm`   | 256      | Encrypt / Decrypt      | `SymmetricKey`  |
| `Ed25519`     | 256      | Sign / Verify          | `KeyPair`       |

---

## 8. Store Modes

| Mode              | Behavior                                      |
|-------------------|-----------------------------------------------|
| `NewOnly`         | Fail if secret already exists at the path      |
| `ReplaceExists`   | Fail if secret does not exist at the path      |
| `CreateOrReplace` | Insert or overwrite unconditionally            |

---

## 9. File Write Safety

Writes use an atomic two-step process to prevent corruption:

1. Serialize the full `StorageFile` to pretty-printed JSON
2. Write to a temporary file (`<path>.tmp`)
3. Call `fsync()` to flush to disk
4. Atomic rename from `<path>.tmp` to `<path>`

This ensures the vault file is never in a partially-written state.

---

## 10. Versioning and Migration

### 10.1 Version History

| Version | Changes                                                              |
|---------|----------------------------------------------------------------------|
| 1       | Initial format                                                       |
| 2       | Fixed Ed25519 key storage: stores 32-byte seed instead of 64-byte expanded key |

### 10.2 Migration Process

When a vault file with an older version is opened:

1. Read and parse JSON
2. Check `version` field against `FORMAT_VERSION` (currently 2)
3. If version is older and `auto_migrate` is enabled:
   - Create timestamped backup: `<path>.backup_v<old_version>_YYYY-MM-DD_HH-MM-SS`
   - Apply sequential migrations (v1 -> v2, etc.)
   - Each migration decrypts affected secrets, transforms data, re-encrypts
   - Safe-save the migrated file
4. If version is newer than supported: error

### 10.3 v1 to v2 Migration

- **Problem:** v1 stored Ed25519 keys as 64-byte expanded keys
- **Fix:** Extract first 32 bytes (seed), re-derive the correct public key, re-encrypt
- Non-Ed25519 secrets are unaffected

---

## 11. Complete Binary Data Layout Diagram

```
 JSON File (on disk)
 ===================
 {
   "version": 2,
   "tree": {
     "children": {
       "<segment>": {
         "secret": {
           "encrypted_data": "<hex>"   ◄── hex-encode of Ciphertext Blob
         }
       }
     }
   }
 }

 Ciphertext Blob (after hex-decode)
 ===================================
 ┌──────────────┬──────────────────────────┬──────────────┐
 │ Nonce (12B)  │ AES-256-GCM Ciphertext   │ GCM Tag (16B)│
 └──────┬───────┴────────────┬─────────────┴──────┬───────┘
        │                    │                     │
        │         Decrypts to Plaintext            │
        │                    ▼                     │
        │  ┌─────────────────────────────────────┐ │
        │  │         Plaintext Buffer             │ │
        │  ├──────────┬──────────┬───────┬───────┤ │
        │  │ data_len │ meta_len │ data  │ meta  │ │
        │  │ (4B LE)  │ (4B LE)  │ (N B) │ (M B) │ │
        │  └──────────┴──────────┴───┬───┴───┬───┘ │
        │                            │       │     │
        │                            ▼       ▼     │
        │            ┌───────────────────┐ ┌──────────────┐
        │            │ KeyMaterial (data) │ │Metadata(JSON)│
        │            ├──────────┬────────┤ └──────────────┘
        │            │sk_len(4B)│pk_len  │
        │            │  (LE)    │(4B LE) │
        │            ├──────────┼────────┤
        │            │secret_key│pub_key │
        │            │ (S B)    │ (P B)  │
        │            └──────────┴────────┘
```
