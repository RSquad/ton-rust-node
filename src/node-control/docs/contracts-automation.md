# Contracts automation (auto-deploy / auto-topup)

The **contracts task** (part of the nodectl service) periodically:

- Deploys the **master** wallet if it is still uninitialized (uses the same deploy path as for validator wallets; minimum balance is tied to the configured wallet deploy value).
- When **auto-deploy** is enabled: deploys **validator wallets** and **nominator pool** contracts (SNP and TONCore use separate deploy amounts) from the **master** wallet.
- When **auto-topup** is enabled: tops up **active** validator wallets when their balance is below a **threshold**, sending a fixed **top-up** amount from the master.
- For **TONCore** pools, may send `update_validator_set` when needed (independent of the `auto_deploy` flag).

Monetary fields in the config file and API are **nanotons** (field names do not repeat the unit); the CLI accepts **TON** and converts to nanotons. Service defaults (if you omit the `automation` block) match the previous hardcoded behaviour: ~1.1 TON deploy, 5 TON threshold, 10 TON top-up, 40 s tick, auto-deploy and auto-topup on.

## Configuration file (`automation`)

Optional block in the root of `nodectl-config.json` / YAML:

```json
"automation": {
  "tick_interval_sec": 40,
  "auto_deploy": true,
  "auto_topup": true,
  "wallet": {
    "deploy": 1100000000,
    "topup": 10000000000,
    "threshold": 5000000000
  },
  "pool": {
    "snp": 1100000000,
    "ton_core": 1100000000
  }
}
```

| Field | Meaning |
|-------|--------|
| `tick_interval_sec` | How often the contracts task runs its loop (1…86400). |
| `auto_deploy` | If `false`, skip deploying validator wallets and pool contracts (master self-deploy is still attempted when needed). |
| `auto_topup` | If `false`, skip topping up validator wallets. |
| `wallet.deploy` | Value (plus fees) sent when deploying a validator wallet; also used for master balance checks before self-deploy. |
| `pool.snp` | Deploy value for a Single Nominator pool contract. |
| `pool.ton_core` | Deploy value for a TONCore pool contract. |
| `wallet.topup` | Amount sent from master when a funded wallet is below the threshold. |
| `wallet.threshold` | Wallets with balance **below** this get a top-up (when `auto_topup` is on). |

Changes applied via REST/CLI are written to the config file on disk and picked up on the next contracts task tick (no service restart required).

## REST API

| Method | Path | Role | Description |
|--------|------|------|-------------|
| `GET` | `/v1/automation/settings` | N (nominator or operator) | Read current settings. |
| `POST` | `/v1/automation/settings` | O (operator only) | Partial update: include only fields to change. |

`POST` body: JSON object with any subset of: `tick_interval_sec`, `auto_deploy`, `auto_topup`, nested `wallet` with `deploy` / `topup` / `threshold`, and nested `pool` with `snp` / `ton_core` (amounts in nanotons). At least one field is required per request.

**Example (curl) — set tick interval and disable auto-topup**

```bash
curl -s -X POST "http://127.0.0.1:8080/v1/automation/settings" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"tick_interval_sec": 60, "auto_topup": false}'
```

**Example — set pool deploy for TONCore only (SNP unchanged)**

```bash
curl -s -X POST "http://127.0.0.1:8080/v1/automation/settings" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"pool": {"ton_core": 2000000000}}'
```

## CLI (requires a running service)

Uses **`nodectl automation`** with the same service URL and JWT flags as `nodectl config …` REST clients: `--url` / `-u`, `--token`, `--config` (see the main [README](../README.md#automation-command)).

**List current settings (table or JSON)**

```bash
nodectl automation ls
nodectl automation ls --format json
```

**Updates (operator JWT if auth is enabled)** — amounts below are in **TON** (not nanotons); each subcommand sends one partial update to the API.

```bash
nodectl automation tick 60

nodectl automation wallet --deploy 1.1 --topup 10 --threshold 5

# Same deploy value for both pool kinds
nodectl automation pool --deploy 1.5

# Shared default with TONCore override
nodectl automation pool --deploy 1.5 --ton-core 2

# Explicit per-kind amounts
nodectl automation pool --snp 1.1 --ton-core 2

nodectl automation enable deploy
nodectl automation enable topup
nodectl automation disable deploy
nodectl automation disable topup
```

Use `nodectl automation <subcommand> --help` for details (`wallet` requires at least one of `--deploy` / `--topup` / `--threshold`; `pool` requires at least one of `--deploy` / `--snp` / `--ton-core`).

## See also

- [Node Control Service Setup](./nodectl-setup.md) — when the service starts and what background tasks do.
- [Security Guide](./nodectl-security.md) — JWT roles (`operator` required to change settings via API / `nodectl automation`).
