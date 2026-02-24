# Changelog

All notable changes to the nodectl Helm chart will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/).
Versions follow the Helm chart release tags (e.g. `helm/nodectl/v0.1.0`).

## [0.1.1] - 2026-02-24

appVersion: `v0.1.0`

### Added

- `storage.resourcePolicy` — configurable `helm.sh/resource-policy` annotation on the PVC, defaults to `keep` to prevent accidental deletion on `helm uninstall`
- `storage.annotations` — extra annotations for the PVC

## [0.1.0] - 2026-02-22

appVersion: `v0.1.0`

Initial release.
