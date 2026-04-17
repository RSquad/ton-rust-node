# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.4.0] - 2026-04-16

### Added

- **TONCore Nominator Pool support** — new pool kind `"core"` with two logical slots (`pools[0]`/`pools[1]`) for even/odd validation rounds. CLI: `config pool add core --even|--odd`, `config pool deposit-validator`, `config pool withdraw-validator`. Runtime routes stakes to the free pool via a dual-pool router and matches stake recovery across both inner pool addresses.
- **Adaptive staking strategy (`adaptive_split50`)** — emulates the Elector's selection algorithm to estimate the minimum effective stake for the current round, then splits half when the remaining half is still competitive and stakes all otherwise. Adds `sleep_period_pct` / `waiting_period_pct` to the `elections` config. See `docs/staking-strategies.md`.
- **Centralised config management through REST API** — all `config` mutations (entity CRUD, settings, logging, TON HTTP API) now flow through JWT-authenticated REST endpoints on the running service, with the CLI acting as a thin client. New endpoints:
  - `POST|DELETE /v1/nodes`, `POST|DELETE /v1/wallets`, `POST|DELETE /v1/pools`, `POST|DELETE /v1/bindings`
  - `POST /v1/elections/settings` (unified stake policy, per-node overrides, `tick_interval`, `max_factor`)
  - `POST /v1/ton-http-api` (with `append` flag for failover endpoints)
  - `POST /v1/log`
  - `GET /v1/elections/settings`, `GET /v1/nodes`, `GET /v1/wallets`, `GET /v1/pools`, `GET /v1/bindings`, `GET /v1/log`, `GET /v1/master-wallet`
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
