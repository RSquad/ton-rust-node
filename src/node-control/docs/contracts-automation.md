# Contracts automation (auto-deploy / auto-topup)

The **contracts monitor** (part of the nodectl service) periodically:

- Deploys the **master** wallet if it is still uninitialized (uses the same deploy path as for validator wallets; minimum balance is tied to the configured wallet deploy value).
- When **auto-deploy** is enabled: deploys **validator wallets** and **nominator pool** contracts (SNP and TONCore use separate deploy amounts) from the **master** wallet.
- When **auto-topup** is enabled: tops up **active** validator wallets when their balance is below a **threshold**, sending a fixed **top-up** amount from the master.
- For **TONCore** pools, may send `update_validator_set` when needed (independent of the `auto_deploy` flag).

All amounts in the config file are in **nanotons**; the CLI accepts **TON** and converts to nanotons. Service defaults (if you omit the `contracts_automation` block) match the previous hardcoded behaviour: ~1.1 TON deploy, 5 TON threshold, 10 TON top-up, 40 s tick, auto-deploy and auto-topup on.

## Configuration file (`contracts_automation`)

Optional block in the root of `nodectl-config.json` / YAML:

```json
"contracts_automation": {
  "tick_interval_sec": 40,
  "auto_deploy": true,
  "auto_topup": true,
  "wallet_deploy_nanotons": 1100000000,
  "pool_deploy_nanotons": {
    "single_nominator": 1100000000,
    "ton_core": 1100000000
  },
  "wallet_topup_nanotons": 10000000000000,
  "wallet_balance_threshold_nanotons": 5000000000
}
```

| Field | Meaning |
|-------|--------|
| `tick_interval_sec` | How often the contracts monitor runs (1…86400). |
| `auto_deploy` | If `false`, skip deploying validator wallets and pool contracts (master self-deploy is still attempted when needed). |
| `auto_topup` | If `false`, skip topping up validator wallets. |
| `wallet_deploy_nanotons` | Value (plus fees) sent when deploying a validator wallet; also used for master balance checks before self-deploy. |
| `pool_deploy_nanotons.single_nominator` | Deploy value for a Single Nominator pool contract. |
| `pool_deploy_nanotons.ton_core` | Deploy value for a TONCore pool contract. |
| `wallet_topup_nanotons` | Amount sent from master when a funded wallet is below the threshold. |
| `wallet_balance_threshold_nanotons` | Wallets with balance **below** this get a top-up (when `auto_topup` is on). |

Changes applied via REST/CLI are written to the config file on disk and picked up on the next monitor tick (no service restart required).

## REST API

| Method | Path | Role | Description |
|--------|------|------|-------------|
| `GET` | `/v1/contracts-automation/settings` | N (nominator or operator) | Read current settings. |
| `POST` | `/v1/contracts-automation/settings` | O (operator only) | Partial update: include only fields to change. |

`POST` body: JSON object with any subset of: `tick_interval_sec`, `auto_deploy`, `auto_topup`, `wallet_deploy_nanotons`, `wallet_topup_nanotons`, `wallet_balance_threshold_nanotons`, and nested `pool_deploy_nanotons` with `single_nominator` and/or `ton_core` (nanotons). At least one field is required per request.

**Example (curl) — set tick interval and disable auto-topup**

```bash
curl -s -X POST "http://127.0.0.1:8080/v1/contracts-automation/settings" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"tick_interval_sec": 60, "auto_topup": false}'
```

**Example — set pool deploy for TONCore only (SNP unchanged)**

```bash
curl -s -X POST "http://127.0.0.1:8080/v1/contracts-automation/settings" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"pool_deploy_nanotons": {"ton_core": 2000000000}}'
```

## CLI (requires a running service)

Uses the same global flags as other `config` subcommands: `--url` / `-u`, `--token`, `--config` (see the main [README](../README.md#global-flags)).

**List current settings (table or JSON)**

```bash
nodectl config contracts-automation ls
nodectl config contracts-automation ls --format json
```

**Update settings (operator JWT if auth is enabled)** — amounts are in **TON** (not nanotons):

```bash
# Slower monitor loop
nodectl config contracts-automation set --tick-interval-sec 60

# Adjust deploy and top-up behaviour
nodectl config contracts-automation set --wallet-deploy 1.1 --wallet-topup 10 --wallet-threshold 5

# Different deploy sizes for SNP vs TONCore pool contracts
nodectl config contracts-automation set --pool-deploy-snp 1.1 --pool-deploy-ton-core 2

# Turn off automatic deploy of wallets/pools (or top-up)
nodectl config contracts-automation set --disable-auto-deploy
nodectl config contracts-automation set --enable-auto-topup
```

You can combine several flags in one `set` call.

## See also

- [Node Control Service Setup](./nodectl-setup.md) — when the service starts and what background tasks do.
- [Security Guide](./nodectl-security.md) — JWT roles (`operator` required to change settings via API/CLI `set`).
