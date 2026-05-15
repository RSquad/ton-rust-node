# Copying Node Secrets from File Storage to HashiCorp Vault

This runbook describes how to migrate a running TON Node's secrets from the
file-based vault to a HashiCorp Vault backend.

## When to run

The recommended moment **is right after elections end** - i.e. once the new 
`config 36` has appeared. New validator keys are only generated during the
elections window, so a copy taken outside that window will not race against
key generation.

The Node needs to be rebooted once during the procedure to pick up the new
`VAULT_URL`.

## Procedure

We assume that:
 - You already have HashiCorp Vault and it is accessible from the Node Pod
 - HashiCorp Vault and the Node Pod are configured for one of the supported
   auth methods: `token` (default) or `k8s` (see README.md in secrets-vault)

### 1. Enter the Pod

Open a shell in the Pod that runs the Node.

### 2. Back up the current file storage

Make a copy of the vault file before doing anything else. The Node can keep
running - the file is read on demand, not held open exclusively.

### 3. Confirm the current `VAULT_URL`

Make sure the Node's current `VAULT_URL` points to the file storage. It must
start with `file:///`, for example:

```
file:///var/ton/vault.json?master_key=<MASTER_KEY_HEX>
```

### 4. Build and deliver the `secrets-vault-cli` binary

Build `secrets-vault-cli` with the `secrets-vault-cli` and `crypto-default`
features:

```bash
cargo build --release --bin secrets-vault-cli \
  --features "secrets-vault-cli crypto-default"
```

The binary is produced at `target/release/secrets-vault-cli`. Copy it into the
Pod, for example with `kubectl cp`:

```bash
kubectl cp target/release/secrets-vault-cli <namespace>/<pod-name>:/tmp/secrets-vault-cli
kubectl exec -n <namespace> <pod-name> -- chmod +x /tmp/secrets-vault-cli
```

The remaining steps assume the binary is on `PATH` inside the Pod (or that you
invoke it via its full path).

### 5. List existing records

From inside the Pod, list everything in the source vault to confirm what is
about to be copied:

```bash
secrets-vault-cli list -f
```

The output looks something like this (truncated):

```
✓ Records: (7)

  ID: validator_keys.WqqHcVbHON5yM2SzVuAlIeg3/LyOnU7DzW8tj7VDoEk=
  Variant: Blob
  Algorithm: None
  Payload: Blob
  Extractable: Yes
  Created At: 2026-05-01 07:53:00 UTC
  Expires At: never
  Tags:
    adnl_key_id = 5iJWG5awkFWTJhxss2qbQzJuQsYFKM2HbtMHvZDmLSg=
    election_id = 1777619866
    expire_at = 1777622280
    type = validator_key
    validator_key_id = WqqHcVbHON5yM2SzVuAlIeg3/LyOnU7DzW8tj7VDoEk=
  ──────────────────────────────────────────────────────────────────
  ID: private_keys.5iJWG5awkFWTJhxss2qbQzJuQsYFKM2HbtMHvZDmLSg=
  Variant: Blob
  Algorithm: None
  Payload: Blob
  Extractable: Yes
  Created At: 2026-05-01 07:53:00 UTC
  Expires At: never
  Tags:
    type = private_key
  ──────────────────────────────────────────────────────────────────
  ...
```

Take note of the record count - it is the number you will verify after the copy.

### 6. Point `FROM_VAULT_URL` at the current file storage

Set `FROM_VAULT_URL` to the **current** value of `VAULT_URL`:

```bash
export FROM_VAULT_URL="$VAULT_URL"
```

Confirm it starts with `file:///`:

```bash
echo "$FROM_VAULT_URL"
```

### 7. Point `VAULT_URL` at the new HashiCorp Vault

Set `VAULT_URL` to the destination URL. Example:

```bash
export VAULT_URL='hashicorp://http://node-vault.node-vault:8200?auth=k8s&auth_mount=k8s-dev&role=validator-0&transit_mount=ton-transit&transit_prefix=validator-0&kv_mount=ton&kv_prefix=validator-0'
```

### 8. Run the copy

```bash
secrets-vault-cli copy
```

Example output (truncated):

```text
Copy:
  from: file:///...
  to:   hashicorp://...

[1/7] READ  private_keys.9NRi96QLmnMTrjHxPpbnbblVIWTBPisd/Eaw4UrYvfs=
         algo=None  payload=Blob  extractable=yes  expires=never  tags=1
         tags: type=private_key
         WRITE validator-0.private_keys.9NRi96QLmnMTrjHxPpbnbblVIWTBPisd/Eaw4UrYvfs=  mode=NewOnly
         OK validator-0.private_keys.9NRi96QLmnMTrjHxPpbnbblVIWTBPisd/Eaw4UrYvfs= (2ms)
 ...
────────────────────────────────────────────────────────────
  total: 7   copied: 7   skipped: 0   failed: 0   elapsed: 29ms

✓ Copy completed
```

Each record is logged with a `READ` line (id, algorithm, payload, extractable,
expiry, tag count and tag values), then a `WRITE` line with the destination
mode, then `OK` on success. The final summary prints
`total / copied / skipped / failed / elapsed`. The command exits non-zero if
any record failed.

If the destination is not empty and you intend to overwrite, add
`--on-conflict overwrite`. To preview without writing, add `--dry-run`.

### 9. Verify the destination

With `VAULT_URL` still pointing at HashiCorp, list the destination:

```bash
secrets-vault-cli list -f
```

Confirm the record count matches what step 5 reported and that the IDs and
tags are present.

### 10. Persist `VAULT_URL` and reboot the Node

The shell-level `export VAULT_URL=...` from step 7 only affects the current
session. The Node still reads the file storage URL from its StatefulSet env.

Update the StatefulSet so that `VAULT_URL` permanently points at HashiCorp,
for example:

```bash
kubectl set env -n <namespace> statefulset/<node-statefulset> \
  VAULT_URL='hashicorp://http://node-vault.node-vault:8200?auth=k8s&auth_mount=k8s-dev&role=validator-0&transit_mount=ton-transit&transit_prefix=validator-0&kv_mount=ton&kv_prefix=dev/validator-0'
```

Then restart the Node Pod so it picks up the new value:

```bash
kubectl rollout restart -n <namespace> statefulset/<node-statefulset>
```

After the Pod comes back up, confirm it is producing blocks / participating
in consensus as before.

## Rollback

If verification fails or the Node misbehaves after restart, revert
`VAULT_URL` in the StatefulSet to the original `file:///` URL and restart the
Pod. Data on the file backend was not modified by the copy, and the backup
from step 2 is your additional safety net.
