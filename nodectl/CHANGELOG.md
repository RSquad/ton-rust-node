# Changelog

All notable changes to nodectl will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the nodectl release tags (e.g. `nodectl/v0.1.0`).

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
