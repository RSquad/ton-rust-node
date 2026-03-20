# Changelog

All notable changes to the Node Control Tool.

## [0.2.0] - 2026-03-04

### Added

- **Wallet V4/V5 support** — added support for wallet contracts v4 and v5, including address verification and code hash validation ([#572](https://github.com/rsquad/ton-node/pull/572))
- **Log file rotation and cleanup** — moved logging configuration into the JSON config file under a `log` section with support for size-based and time-based rotation, configurable max file count and size, and three output modes: `console`, `file`, `all` ([#547](https://github.com/rsquad/ton-node/pull/547))
- **Multi-endpoint failover for ton-http-api** — round-robin failover across multiple RPC endpoints with per-endpoint API keys; new `ton-http-api add` CLI command to append failover endpoints ([#579](https://github.com/rsquad/ton-node/pull/579))
- **Pool address and balance display** — `config pool ls` resolves pool contract addresses and shows live balances via TON HTTP API; added pool and owner address validation ([#589](https://github.com/rsquad/ton-node/pull/589))
- **Log settings CLI** — new `config log ls` and `config log set` commands for viewing and updating log level, rotation, output mode, and file path without editing the config file manually
- **Control server connection status** — `config node ls` shows each node's control server connection status (e.g. "OK" or an error message), using concurrent ADNL pings with a 5-second timeout ([#597](https://github.com/rsquad/ton-node/pull/597))
- **Vault hot reload** — the vault is now reopened on each config reload so newly added secrets are picked up without a service restart; config and dynamic state swap is atomic ([#592](https://github.com/rsquad/ton-node/pull/592))
- **Missing vault secret warnings** — `config node add` and `config wallet add` now warn when the specified secret name does not exist in the vault ([#593](https://github.com/rsquad/ton-node/pull/593))
- **Balance parsing from string or integer** — new `u64_as_str_or_num` serde helper to accept `u64` values serialized as either JSON string or number, fixing TonCenter API v2 balance deserialization ([#591](https://github.com/rsquad/ton-node/pull/591))

### Changed

- **Migrated to Axum web framework** — replaced the previous web framework with Axum for the control HTTP server ([#576](https://github.com/rsquad/ton-node/pull/576))
- **Removed `--verbose` and `--log-file` CLI flags** — log level can still be overridden via the `RUST_LOG` environment variable ([#547](https://github.com/rsquad/ton-node/pull/547))
- **Backward-compatible config migration** — old `"url": "…"` field in ton-http-api config is transparently migrated to `urls` on first re-save ([#579](https://github.com/rsquad/ton-node/pull/579))

### Fixed

- **Deploy only unique wallets** — when a single wallet is configured for multiple nodes, it is now deployed only once ([41302b57](https://github.com/rsquad/ton-node/commit/41302b57))
- **Wallet send `--bounce` flag and confirmation default** — fixed `--bounce` flag handling and clarified the default confirmation prompt ([#607](https://github.com/rsquad/ton-node/pull/607))
- **Wallet version help text case insensitivity** — updated wallet version help text to reflect case-insensitive input ([#608](https://github.com/rsquad/ton-node/pull/608))

### Docs

- Updated nodectl documentation to reflect current configuration and usage ([#583](https://github.com/rsquad/ton-node/pull/583))

## [0.1.1] - 2026-02-27

### Added

- **Wallet V1R3 support** - added support for wallet contract v1r3

## [0.1.0] - 2026-02-22

Initial release.
