# Changelog

All notable changes to nodectl will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the nodectl release tags (e.g. `nodectl/v0.1.0`).

## [v0.2.0] - 2026-03-05

Image: `ghcr.io/rsquad/ton-rust-node/nodectl:v0.2.0`

### Added
- `config log` CLI commands (`ls`, `set`) for viewing and updating log settings
- Log file rotation and automatic cleanup configuration
- Support for multiple ton-http-api endpoints with failover and per-endpoint API keys
- Support for V4 and V5 wallet contracts
- Control server connection status in `config node ls` output
- Auto-detection of pool addresses and balances in `config pool ls`
- Owner address validation in `config pool add`
- Warning on missing node key in vault during `config node add` / `config pool add`
- Automated single-host test network script

### Changed
- REST API rewritten using Axum framework
- Vault is now reopened automatically on configuration reload
- `--version` parameter for `wallet add` is now case-insensitive

### Fixed
- `wallet send`: broken `--bounce` flag and unclear confirmation default
- Stake amount mismatch between calculated and submitted values
- Balance parsing error when using ton-http-api
- Duplicate wallet deployment when a single wallet is shared across nodes

## [v0.1.1] - 2026-02-27

Image: `ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.1`

### Added
- Support for V1R3 wallet type in `config-wallet` command (`--version V1R3`)
- V1R3 wallet contract: address computation, state init, and external message building

### Changed
- `subwallet_id` has no effect for V1R3 wallets (V1R3 does not have a subwallet concept)
- Wallet version help text now lists all supported versions (`V1R3`, `V3R2`)

## [v0.1.0] - 2026-02-22

Image: `ghcr.io/rsquad/ton-rust-node/nodectl:v0.1.0`

Initial release.
