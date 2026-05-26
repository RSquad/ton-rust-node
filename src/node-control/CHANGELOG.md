# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.5.1] - 2026-05-25

### Added

- **Lagging JSON-RPC endpoint detection** — the ton-http-api client now checks endpoint freshness before each request by reading the masterchain tip and comparing its timestamp against wall-clock time. Endpoints whose chain view exceeds the configured lag threshold are skipped, preventing stale data from a lagging RPC from propagating into election decisions.

### Changed

- **Priority-based JSON-RPC endpoint failover** — the ton-http-api client now follows strict priority order instead of round-robin: the first configured endpoint is the primary and carries all traffic when healthy; subsequent entries are fallbacks used only on error or when the primary is detected as lagging.

### Fixed

- **Past-elections cache refreshed periodically** — the elections runner now refreshes its past-elections and pool-address caches on a TTL instead of only invalidating when the election round changes. Prevents a stale snapshot from a lagging RPC endpoint from persisting for an entire round and distorting frozen-stake accounting.
- **Nominator Pool active slot stays the same for the whole election cycle** — for two-pool Nominator Pool setups, the router could switch to the idle sibling pool a few minutes after stake submission once the pool-address cache refreshed, so subsequent balance and stake-recovery lookups acted on the wrong pool. The active slot is now pinned to the cycle being serviced and stays consistent until the next election cycle.
- **Nominator Pool: skip redundant validator-set update transactions** — the contracts task no longer sends `update_validator_set` to a Nominator Pool when the on-chain validator set has not changed since the last update. Removes a no-op masterchain transaction on every automation tick.

## [0.5.0] - 2026-05-18

### Added

- **Nominator Pool deploy mode** — new per-slot setting that controls how a Nominator Pool is deployed on first run. Two values: **`legacy`** (the way older nodectl versions deployed; addresses match existing pools) and **`tonscan`** (recognised by Tonscan and other explorers — recommended for all new pools). Already-deployed pools must stay on the mode they were created with — switching changes the derived address. Available in the JSON config, REST (`POST /v1/pools/core`), and CLI (`nodectl config pool add core --deploy-mode legacy|tonscan`). Defaults: existing pools in saved config without the field stay on `legacy`; new pools created via REST or CLI default to `tonscan`.
- **`DELETE /v1/elections/static-adnl/{node}`** — opt a node out of the new static-ADNL default; the runner will generate a fresh ephemeral ADNL each cycle (pre-v0.5 behavior).
- **`--disable` flag on `nodectl config elections static-adnl`** — CLI equivalent of the DELETE endpoint. Running the existing rotate command again re-enables the static default.
- **`static_adnl_disabled` shown in elections settings** — `GET /v1/elections/settings` and `nodectl config elections ls` now indicate which nodes are opted out of static ADNL (a "disabled" marker appears in the Static ADNL column).
- **`nodectl config elections wait`** — new subcommand (alias `wait-pct`) to set the AdaptiveSplit50 staking window: `--min` sets `sleep_period_pct` (earliest stake submission, as fraction of election duration; default 0.2), `--max` sets `waiting_period_pct` (latest peer-wait deadline; default 0.4, must be ≥ `--min`). Both values are now also shown in `nodectl config elections ls`. Only applied under the `adaptive_split50` stake policy.
- **Contracts automation: auto-deploy and auto-topup** — new `automation` config section to enable automatic wallet/pool deploy and balance top-up, with separate toggles for SNP vs Nominator Pool deploy, configurable amounts (in TON) and tick interval. Manage live via `GET|POST /v1/automation/settings` or `nodectl config automation ls|set` (no service restart). See `docs/automation.md`.
- **`NODECTL_API_CONNECT_TIMEOUT_SECS` / `NODECTL_API_REQUEST_TIMEOUT_SECS`** — env overrides for the nodectl CLI's REST connect timeout (default 10 s) and overall request timeout (default 60 s). Timeout and connect failures now produce an actionable error message that includes the URL that was attempted.
- **In-pod vault migration via `nodectl key migrate`** — new CLI command that copies all secrets from a file vault (`FROM_VAULT_URL=file://...`) to a HashiCorp vault (`VAULT_URL=hashicorp://...`) without leaving the Pod and without any extra binary. Supports `--dry-run`, `--on-conflict <fail|skip|overwrite>`, and `--continue-on-error`. Non-extractable secrets are migrated too — the original `extractable` flag is preserved on the destination, so a wallet key that was non-extractable in the source vault remains non-extractable in HashiCorp. Source must be `file://` and destination must be `hashicorp://`; any other combination is rejected up front. See `helm/nodectl/docs/copy-file-to-hashicorp.md`.

### Changed

- **Static ADNL is now the default across elections** — previously nodectl generated a fresh ADNL address every election cycle. The new default aligns nodectl with the behavior of mytonctrl-managed C++ nodes, which reuse the same ADNL across all validation rounds; a single persistent ADNL address also enables fastsync for the Rust node. nodectl now generates and persists a static ADNL per node on the first election cycle and reuses it thereafter. Existing `elections.static_adnls` entries are honored unchanged. If a node's control server is briefly unreachable during generation, the rest of the nodes proceed normally and the failing one retries next tick. Use the new `--disable` flag (or `DELETE /v1/elections/static-adnl/{node}`) to opt a node back into pre-v0.5 per-cycle behavior.
- **Nominator Pool: process pending withdraws before each new stake** — before submitting a new stake, the elections runner now checks the active TONCore pool for pending nominator withdraw requests. If any are queued, it triggers the pool to process them and skips staking for that tick so the pool can drain; the next tick re-checks and either continues draining or proceeds to stake. This frees up locked liquidity from nominators who already requested a withdrawal so it does not get re-staked. A new participant status `processing_withdraw_requests` is surfaced in `/v1/elections` and `/v1/validators` snapshots. No-op for SNP nominator pools and direct staking.
- **Nominator Pool validator deposit fee is now added automatically** — `config pool deposit-validator` sends `stake + 1 TON processing fee` so the requested stake amount actually arrives at the pool. Previously operators had to account for the 1 TON fee themselves.
- **`config pool ls` shows TONCore pools as `not deployed` instead of an RPC error** — when a TONCore pool contract is uninitialized or not yet on-chain, the row now reads `not deployed` and the configured pool parameters from local config are still shown so you can see the planned layout before deploy.
- **Voting moved to the REST API** (SMA-85, #132) — config-proposal voting is now served by `GET /v1/voting/config`, `GET /v1/voting/proposals`, `GET /v1/voting/proposals/{hash}`, `POST /v1/voting/proposals`, `DELETE /v1/voting/proposals/{hash}` (reads available to nominators and operators; mutations operator-only). `nodectl vote ls|inspect|add|rm` is now a thin REST client and uses the same `--url` / token / config resolution as other REST commands — the service must be running, same as other `config` mutations since v0.4.0.
- **`config wallet ls` always shows bounceable URL-safe base64 addresses**  — table and JSON output stay consistent regardless of how the service serializes addresses internally.
- **`--validator-share-percent` on `nodectl config pool add core`** — accept the Nominator Pool validator share as a human percent (e.g. `40`) instead of basis points.

### Fixed

- **Concurrent `config elections enable/include/exclude` no longer spawns duplicate election tasks** — previously, two concurrent HTTP-triggered restarts of an election task could race such that one task was orphaned and kept running until the process restarted. Restarts now queue and execute one at a time.
- **nodectl service no longer becomes unresponsive when a node's control-server is unreachable** — a firewalled or black-holed control port used to block tokio worker threads on connect, which could cause `/health`, `/v1/nodes`, and other REST endpoints to stop responding. Connects are now non-blocking and time out cleanly without stalling the rest of the service.

### Internal

- **`elections` crate merged into `service` as a module** — code moved under `service/src/elections/`; the standalone `node-control/elections` workspace member is removed. No behavior change for operators.
- **CI: highload wallet for Nominator Pool setup** — speeds up multi-nominator load test bootstrap; single-host "snp-toncore" scenario node count lowered from 7 to 5 (default scenario has 6 nodes as before).
- **REST entity CRUD tests** — automated coverage for `POST|DELETE /v1/{nodes,wallets,pools,bindings}`: happy paths, validation/conflict cases, persistence to disk, role checks (nominator → 403, operator → allowed).

## [0.4.0] - 2026-04-21

### Added

- **Nominator Pool support** — nodectl now supports TON Core Nominator Pool contracts. This pool type uses a pair of pools that alternate between even and odd validation rounds. Add each pool with `config pool add core --even` / `--odd`, then manage the validator deposit with `config pool deposit-validator` and `config pool withdraw-validator`. The election runner automatically picks the available pool each round and tracks stake recovery from both.
- **Adaptive staking strategy (`adaptive_split50`)** — emulates the Elector's selection algorithm to estimate the minimum effective stake for the current round, then splits half when the remaining half is still competitive and stakes all otherwise. Adds `sleep_period_pct` / `waiting_period_pct` to the `elections` config. See `docs/staking-strategies.md`.
- **Centralised config management through REST API** — all `config` mutations (entity CRUD, settings, logging, TON HTTP API) now flow through JWT-authenticated REST endpoints on the running service, with the CLI acting as a thin client. New endpoints:
  - `POST|DELETE /v1/nodes`, `POST|DELETE /v1/wallets`, `POST|DELETE /v1/pools`, `POST|DELETE /v1/bindings`
  - `POST /v1/elections/settings` (unified stake policy, per-node overrides, `tick_interval`, `max_factor`)
  - `POST /v1/ton-http-api` (with `append` flag for failover endpoints)
  - `POST /v1/log`
  - `GET /v1/elections/settings`, `GET /v1/nodes`, `GET /v1/wallets`, `GET /v1/pools`, `GET /v1/bindings`, `GET /v1/log`, `GET /v1/master-wallet`
- **Persistent ADNL address across elections** — validators can now keep the same ADNL address across election cycles instead of generating a fresh one each time. New `elections.static_adnls` config map stores pre-generated ADNL key hashes per node (base64). New `POST /v1/elections/static-adnl` endpoint and `nodectl config elections static-adnl --node <name>` CLI command generate the key on the validator node and save it to config. The election runner re-registers the stored address each cycle via `add_validator_adnl_addr`.
- **Voting CLI (`nodectl vote`)** — `ls`, `inspect`, `add`, `rm` subcommands to view on-chain config proposals and manage the voting task's tracked-proposals list.
- **Reserved `master_wallet` name** — `config wallet add` rejects the reserved name `master_wallet`, and `config wallet rm master_wallet` fails immediately with a clear error instead of attempting to mutate the master wallet slot.

### Changed

- **`max_factor` upper bound read from the network** — instead of the hardcoded `3.0`, nodectl now reads the limit from masterchain config param 17 (`max_stake_factor`).
- **Unified elections settings endpoint** — `POST /v1/elections/settings` replaces the removed `/v1/stake_strategy`, `/v1/elections/tick-interval`, and `/v1/elections/max-factor`. Accepts any combination of `policy`, `node`, `reset`, `tick_interval`, `max_factor` in one request.
- **Unified TON HTTP API endpoint** — `POST /v1/ton-http-api` replaces the separate `set`/`add` endpoints; pass `append: true` to keep existing URLs.

### Breaking Changes

- **Removed `POST /v1/stake_strategy`** — use `POST /v1/elections/settings` with `{"policy": ...}`.
- **Removed `config stake-policy` top-level alias** — use `config elections stake-policy`.
- **`config ton-http-api set --url` → `--endpoint`** — the flag was renamed (short form `-e`) to disambiguate from the root `--url` service-URL flag introduced for REST client commands. Update any scripts invoking `nodectl config ton-http-api --url ...`.
- **Configuration mutations require a running service** — `config {node,wallet,pool,bind,elections,log,ton-http-api,master-wallet}` subcommands are now REST clients and need the service to be running with an operator user. Only `config generate` still writes to disk directly.

### Fixed

- **master_wallet duplication / deletion** — reserved the logical name `master_wallet` so it cannot collide with a regular wallet entry.
- **next elections range in `/v1/elections` response** - fixed calculation of next elections range in `/v1/elections` response.
- **validator snapshot sourced from elections data instead of current vset** — `adnl`, `pubkey`, `key_id`, `key_election_id`, `key_expires_at`, and `stake` in `/v1/validators` were pulled from the pending election bid rather than the active validator set (p34) and `past_elections` frozen map, showing stale values when a node was validating and bidding for the next round simultaneously.

## [0.3.0] - 2026-03-24

### Added

- **JWT-based authentication for REST API** — login, token revocation, auth middleware with login rate limiter, argon2 password hashing, and `NODECTL_API_TOKEN` env support; new `auth` and `api login` CLI commands
- **Election status dashboard** — `/v1/elections` API endpoint and `nodectl api elections` CLI table with participation lifecycle tracking (Idle → Participating → Submitted → Accepted → Elected → Validating), stake sums, and election metadata
- **Validation keys listing** — `/v1/validators` API endpoint and `nodectl api validators` command displays validator information including validator key with election ID, created/expires timestamps, validator status, key ID, and ADNL address
- **Kubernetes internal DNS support** — control server address now accepts DNS names (e.g. `validator-0-control.ton.svc.cluster.local`) in addition to IP addresses
- **JWT authorization in Swagger UI** — added `bearerAuth` security scheme to OpenAPI spec; Swagger UI now shows an "Authorize" button for Bearer token authentication
- **`--filter` for elections and validators API** — `nodectl api elections` and `nodectl api validators` accept `--filter=<name>` to limit output to specific nodes
- **`--format=json|table` flag** — added `--format=json|table` flag to all `config ... ls` subcommands (`config bind ls`, `config elections ls`, `config log ls`, `config node ls`, `config pool ls`, `config wallet ls`, `master-wallet ls`)

### Changed

- **Bounceable base64 wallet addresses** — `config wallet ls` now displays addresses in bounceable URL-safe base64 format
- **Improved endpoint round-robin** — lowered retry loop log level to debug, shortened error messages when all endpoints fail, fixed `rr_cursor` initial value starting from 1 instead of 0
- **Graceful RPC error handling in `wallet ls`** — wallet listing no longer fails when TON API is unreachable; addresses are still displayed with `-` for unavailable state/balance fields; unified warning format
- **Hot reload for auth state** — JWT TTL changes, newly added users, and token revocations take effect on config reload without service restart; JWT signing key is generated on first start even if auth is disabled
- **Extended version command** — `nodectl --version` now prints build artifacts (git hash, build date, features)
- **Updated documentation** — added descriptions for new commands and flags, fixed documentation errors, added document on `nodectl` security model, added documentation for first elections with Rust node, added documentation for REST API authentication

### Fixed

- **`State` column alignment in `wallet ls`** — adjusted column width to fix misalignment in `config wallet ls` output
- **Missing OpenAPI schema references** — registered `ElectionsStatus`, `NodeListRequest`, and nested election schemas (`OurElectionParticipant`, `ParticipationStatus`, `StakeSubmission`) in OpenAPI components, fixing Swagger resolver errors

## [0.2.1] - 2026-03-18

### Added

- **Manual election stake command** — new `config wallet stake` command for manual election participation via nominator pool.

## [0.2.0] - 2026-03-04

### Added

- **Wallet V4/V5 support** — added support for wallet contracts v4 and v5, including address verification and code hash validation
- **Log file rotation and cleanup** — moved logging configuration into the JSON config file under a `log` section with support for size-based and time-based rotation, configurable max file count and size, and three output modes: `console`, `file`, `all`
- **Multi-endpoint failover for ton-http-api** — round-robin failover across multiple RPC endpoints with per-endpoint API keys; new `ton-http-api add` CLI command to append failover endpoints
- **Pool address and balance display** — `config pool ls` resolves pool contract addresses and shows live balances via TON HTTP API; added pool and owner address validation
- **Log settings CLI** — new `config log ls` and `config log set` commands for viewing and updating log level, rotation, output mode, and file path without editing the config file manually
- **Control server connection status** — `config node ls` shows each node's control server connection status (e.g. "OK" or an error message), using concurrent ADNL pings with a 5-second timeout
- **Vault hot reload** — the vault is now reopened on each config reload so newly added secrets are picked up without a service restart; config and dynamic state swap is atomic
- **Missing vault secret warnings** — `config node add` and `config wallet add` now warn when the specified secret name does not exist in the vault
- **Balance parsing from string or integer** — new `u64_as_str_or_num` serde helper to accept `u64` values serialized as either JSON string or number, fixing TonCenter API v2 balance deserialization

### Changed

- **Migrated to Axum web framework** — replaced the previous web framework with Axum for the control HTTP server
- **Removed `--verbose` and `--log-file` CLI flags** — log level can still be overridden via the `RUST_LOG` environment variable
- **Backward-compatible config migration** — old `"url": "…"` field in ton-http-api config is transparently migrated to `urls` on first re-save
- Updated nodectl documentation to reflect current configuration and usage

### Fixed

- **Deploy only unique wallets** — when a single wallet is configured for multiple nodes, it is now deployed only once
- **Wallet send `--bounce` flag and confirmation default** — fixed `--bounce` flag handling and clarified the default confirmation prompt
- **Wallet version help text case insensitivity** — updated wallet version help text to reflect case-insensitive input

## [0.1.1] - 2026-02-27

### Added

- **Wallet V1R3 support** — added support for wallet contract v1r3

## [0.1.0] - 2026-02-22

Initial release.
