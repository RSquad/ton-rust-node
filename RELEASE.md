# Release Guidelines

## Versioning

This project follows [Semantic Versioning](https://semver.org/): `MAJOR.MINOR.PATCH`.

Version tags include an artifact prefix and `v`: `node/v1.2.0`, `helm/node/v0.3.0`.
A network qualifier may be appended as a pre-release suffix: `node/v0.1.2-mainnet`.

The Helm chart is versioned independently from the node.

| Artifact       | Tag format                | Examples                                  |
|----------------|---------------------------|-------------------------------------------|
| Node           | `node/v<semver>`          | `node/v1.2.0`, `node/v0.1.2-mainnet`     |
| Helm chart     | `helm/node/v<semver>`     | `helm/node/v0.2.2`                        |
| nodectl        | `nodectl/v<semver>`       | `nodectl/v0.1.0`                           |
| nodectl chart  | `helm/nodectl/v<semver>`  | `helm/nodectl/v0.1.0`                     |

## Commits

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope): short description`

**Types:** `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`

**Scopes:** `helm`, `nodectl`, `grafana` (more scopes will appear as the node code lands)

## Branches

| Branch              | Purpose                                                              |
|---------------------|----------------------------------------------------------------------|
| `master`            | Production-ready code. All final release tags live here.             |
| `release/<version>` | Stabilization branch for an upcoming release. Branched off `master`. |
| `hotfix/<version>`  | Urgent fix for a previously released version. Branched off the relevant release tag. |

The `<version>` in branch names matches the target tag, e.g. `release/node/v0.2.0-mainnet`,
`hotfix/node/v0.1.3-mainnet`, `release/helm/node/v0.3.0`.

## Tags

### Node Tags

| Tag                     | Meaning            | Placed on  |
|-------------------------|--------------------|------------|
| `node/v1.2.0`           | Final release      | `master`   |
| `node/v0.1.2-mainnet`   | Final release (network qualifier) | `master` |
| `node/v1.2.0-rc.1`      | Release candidate  | `release/*` |
| `node/v0.1.3-mainnet`   | Hotfix for older version | `hotfix/*` (if not latest) |

### Helm Chart Tags

| Tag                   | Meaning       | Placed on |
|-----------------------|---------------|-----------|
| `helm/node/v0.2.2`   | Final release | `master`  |

### nodectl Tags

| Tag                     | Meaning              | Placed on   |
|-------------------------|----------------------|-------------|
| `nodectl/v0.1.0`        | Final release        | `master`    |
| `helm/nodectl/v0.1.0`   | Chart final release  | `master`    |

### Rules

- Final release tags live on `master`, except hotfixes for older versions.
- Release candidate tags live on `release/*` branches.
- Node, Helm chart, and nodectl tags are independent and may point to the same or different commits.

## Releases and Hotfixes

### Standard Release

Create a branch `release/<version>` from `master`. Stabilize, tagging release
candidates as `node/v<version>-rc.N` or `helm/node/v<version>-rc.N`. Once stable, merge into `master` and tag.

### Hotfix (latest version)

Create a feature branch from `master`, fix, merge via PR, tag on `master`.

### Hotfix (older version)

Create `hotfix/<version>` from the relevant release tag. Fix and tag on that
branch. Cherry-pick into `master` if applicable. GitHub Release is published
with `make_latest: false`.

## CI Pipeline

### On Node Tags (`node/v*`)

1. Build and push container images to `ghcr.io/rsquad/ton-rust-node/node`.
2. Create a GitHub Release.

| Git Tag                        | Docker Tags                      | GitHub Release  |
|--------------------------------|----------------------------------|-----------------|
| `node/v1.2.0-mainnet`         | `v1.2.0-mainnet`, `latest`       | Latest          |
| `node/v1.2.0-rc.1`            | `v1.2.0-rc.1`                    | Pre-release     |
| `node/v0.1.3-mainnet` (hotfix)| `v0.1.3-mainnet` (no `latest`)   | `latest: false` |

### On Helm Tags (`helm/node/v*`)

1. Package the Helm chart.
2. Push to `oci://ghcr.io/rsquad/ton-rust-node/helm/node`.
3. Create a GitHub Release.

| Git Tag              | OCI Tag  | GitHub Release  |
|----------------------|----------|-----------------|
| `helm/node/v0.2.2`  | `0.2.2`  | `latest: false` |

### On nodectl Tags (`nodectl/v*`)

1. Build and push container images to `ghcr.io/rsquad/nodectl` (upstream). Mirror to `ghcr.io/rsquad/ton-rust-node/nodectl`.
2. Create a GitHub Release.

| Git Tag            | Docker Tags              | GitHub Release  |
|--------------------|--------------------------|-----------------|
| `nodectl/v0.1.0`  | `v0.1.0`, `latest`       | `latest: false` |

### On nodectl Helm Tags (`helm/nodectl/v*`)

1. Package the nodectl Helm chart.
2. Push to `oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl`.
3. Create a GitHub Release.

| Git Tag                 | OCI Tag  | GitHub Release  |
|-------------------------|----------|-----------------|
| `helm/nodectl/v0.1.0`  | `0.1.0`  | `latest: false` |

## GitHub Releases

- Only node releases on `master` are marked as **Latest**.
- Helm chart releases are always published with `make_latest: false`.
- nodectl and nodectl chart releases are always published with `make_latest: false`.
- Release candidates are marked as **Pre-release**.
- Hotfixes for older versions are published with `make_latest: false`.

## PR Guidelines

- Use meaningful PR titles — they serve as the changelog.
- Apply labels (`enhancement`, `bug`, `infrastructure`, `dependencies`) for grouped release notes.
- Add `skip-changelog` to PRs that should not appear in release notes.

