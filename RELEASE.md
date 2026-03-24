# Release Guidelines

## Versioning

This project follows [Semantic Versioning](https://semver.org/): `MAJOR.MINOR.PATCH`.

Version tags include an artifact prefix and `v`: `node/v1.2.0`, `helm/node/v0.3.0`.
A network qualifier may be appended as a pre-release suffix: `node/v0.1.2-mainnet`.

Each artifact is versioned independently.

| Artifact       | Tag format                | Examples                                  |
|----------------|---------------------------|-------------------------------------------|
| Node           | `node/v<semver>`          | `node/v1.2.0`, `node/v0.1.2-mainnet`     |
| nodectl        | `nodectl/v<semver>`       | `nodectl/v0.3.0`                          |
| Helm chart     | `helm/node/v<semver>`     | `helm/node/v0.2.2`                        |
| nodectl chart  | `helm/nodectl/v<semver>`  | `helm/nodectl/v0.1.0`                     |

## Commits

[Conventional Commits](https://www.conventionalcommits.org/): `type(scope): short description`

**Types:** `feat`, `fix`, `docs`, `chore`, `refactor`, `test`, `ci`

**Scopes:** `helm`, `nodectl`, `grafana`, `node`

## Branches

| Branch              | Purpose                                                              |
|---------------------|----------------------------------------------------------------------|
| `master`            | Production-ready code. All final release tags live here.             |
| `release/<artifact>/<version>` | Release branch. Features and changes are merged here, then the branch is merged into `master` for release. |
| `hotfix/<version>`  | Urgent fix for a previously released version. Branched off the relevant release tag. |

This is a monorepo with independent release cycles for each product (node, nodectl). Release branches are product-scoped:

- `release/node/v0.2.0-mainnet`
- `release/nodectl/v0.3.0`
- `release/helm/node/v0.4.0`

## Release Process

### 1. Create release branch

Branch off `master`:

```bash
git checkout master && git pull
git checkout -b release/<artifact>/v<version>
```

### 2. Develop on the release branch

Merge feature branches and fixes into the release branch. This is where all changes for the release are collected.

### 3. Test with RC tags (optional)

Tag release candidates **only from the release branch** to trigger CI:

```bash
git tag <artifact>/v<version>-rc.1
git push origin <artifact>/v<version>-rc.1
```

CI builds and publishes the RC automatically. RC releases are marked as **Pre-release** on GitHub.

> **Internal/test builds:** If you need to build an image for testing outside of a release branch, use the `alpha` or `beta` pre-release suffix: `<artifact>/v<version>-alpha.1`. These builds are strictly internal — never distribute or deploy them to production.

### 4. Open PR into master

When the release is ready, open a PR from the release branch into `master`:
- **PR title:** `release/<artifact>:v<version>` (e.g. `release/nodectl:v0.3.0`)
- **PR body:** changelog entry (without the `## [version]` header)

### 5. Merge into master

Squash-merge the PR:

```bash
gh pr merge <N> --squash --admin
```

### 6. Tag final release

```bash
git checkout master && git pull
git tag <artifact>/v<version>
git push origin <artifact>/v<version>
```

### 7. CI publishes automatically

CI triggers on the tag and handles everything:
- Builds artifacts (containers, binaries, Helm charts)
- Pushes to registries
- Creates a GitHub Release

### 8. Finalize GitHub Release

CI creates the release, but the body may need updating. Edit with the changelog entry:

```bash
gh release edit <tag> -F- <<< '<changelog entry>'
```

For **node releases only**, Latest is default. All other artifacts:

```bash
gh release edit <tag> --latest=false
```

Verify:

```bash
gh release list   # only node release should be Latest
```

### 9. Cleanup

```bash
git branch -d release/<artifact>/v<version>
git push origin --delete release/<artifact>/v<version>
```

## Helm Chart Releases

Helm charts should be released **together with their parent app** (node or nodectl), bumping `appVersion` and `image.tag` to match.

Release Helm charts independently **only** for chart-specific bugfixes (template fixes, value changes) that don't involve an app version change.

When releasing together:
1. Include Helm chart changes in the same release branch as the app.
2. After the app release is tagged, tag the Helm chart separately: `helm/node/v<version>`.
3. CI packages and publishes the chart automatically.

## CI Pipelines

### On Node Tags (`node/v*`)

1. Build container image from `src/Dockerfile`.
2. Push to `ghcr.io/rsquad/ton-rust-node/node`.
3. Create GitHub Release.

| Git Tag                 | Docker Tags                | GitHub Release |
|-------------------------|----------------------------|----------------|
| `node/v1.2.0-mainnet`  | `v1.2.0-mainnet`, `sha-*`  | Latest         |
| `node/v1.2.0-rc.1`     | `v1.2.0-rc.1`, `sha-*`     | Pre-release    |

### On nodectl Tags (`nodectl/v*`)

1. Build cross-platform binaries (linux/amd64, linux/arm64, darwin/arm64, windows/amd64).
2. Build container image from `src/Dockerfile.nodectl`.
3. Push image to `ghcr.io/rsquad/ton-rust-node/nodectl`.
4. Create GitHub Release with binaries attached.

| Git Tag              | Docker Tags                    | Binaries                | GitHub Release  |
|----------------------|--------------------------------|-------------------------|-----------------|
| `nodectl/v0.3.0`    | `v0.3.0`, `latest`, `sha-*`   | linux, darwin, windows  | `latest: false` |
| `nodectl/v0.3.0-rc.1` | `v0.3.0-rc.1`, `sha-*`      | linux, darwin, windows  | Pre-release     |

### On Helm Tags (`helm/node/v*`, `helm/nodectl/v*`)

1. Package the Helm chart.
2. Push to OCI registry (`oci://ghcr.io/rsquad/ton-rust-node/helm/<chart>`).
3. Create GitHub Release with chart archive attached.

| Git Tag               | OCI Tag | GitHub Release  |
|-----------------------|---------|-----------------|
| `helm/node/v0.2.2`   | `0.2.2` | `latest: false` |
| `helm/nodectl/v0.1.0`| `0.1.0` | `latest: false` |

### On Pull Requests

CI runs on PRs targeting `master` or `release/**`:

- `audit` — cargo security audit
- `fmt` — formatting check (nightly rustfmt)
- `check` — clippy + cargo check
- `tests` — cargo tests
- `tests-net` — network integration tests

## Registries

| Artifact | Registry |
|----------|----------|
| Node image | `ghcr.io/rsquad/ton-rust-node/node:<tag>` |
| nodectl image | `ghcr.io/rsquad/ton-rust-node/nodectl:<tag>` |
| Node Helm chart | `oci://ghcr.io/rsquad/ton-rust-node/helm/node` |
| nodectl Helm chart | `oci://ghcr.io/rsquad/ton-rust-node/helm/nodectl` |

## GitHub Releases

- Only **node** releases on `master` are marked as **Latest**.
- All other releases (Helm charts, nodectl) use `make_latest: false`.
- Release candidates are marked as **Pre-release**.
- Hotfixes for older versions use `make_latest: false`.

## Changelogs

| Changelog | Artifact | Versioned by |
|-----------|----------|--------------|
| `CHANGELOG.md` | Node | `node/v*` tags |
| `nodectl/CHANGELOG.md` | nodectl | `nodectl/v*` tags |
| `helm/ton-rust-node/CHANGELOG.md` | Node Helm chart | `helm/node/v*` tags |
| `helm/nodectl/CHANGELOG.md` | nodectl Helm chart | `helm/nodectl/v*` tags |

All changelogs use [Keep a Changelog](https://keepachangelog.com/) format.

## Hotfixes

### Hotfix (latest version)

Create a feature branch from `master`, fix, merge via PR, tag on `master`.

### Hotfix (older version)

Create `hotfix/<version>` from the relevant release tag. Fix and tag on that
branch. Cherry-pick into `master` if applicable. GitHub Release is published
with `make_latest: false`.
