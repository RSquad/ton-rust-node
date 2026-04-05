# Sync Test

Automated mainnet sync test for the TON Rust Node. Builds a node image from the current commit, deploys it to Kubernetes via the public [`ton-rust-node`](https://github.com/rsquad/ton-rust-node) Helm chart, and waits for the node to fully sync with the network. Reports the result as a GitHub commit status on the triggering commit.

## How it works

### Overview

```
GitHub Actions (manual trigger, ~5 min)
  1. Build node image → ghcr.io/rsquad/ton-node:sha-<commit>
  2. Set GitHub commit status → pending
  3. helm upgrade --install → deploys to ton-synctest namespace
  4. CI exits

Kubernetes pod (runs for hours)
  Container "ton-node": syncs with mainnet
  Container "watcher":  polls metrics, reports result to GitHub
```

### Watcher logic

The watcher runs as a sidecar container alongside the node. Every 60 seconds it fetches the node's Prometheus metrics and checks sync progress.

**Metrics used:**

| Metric | Description |
|--------|-------------|
| `ton_node_engine_sync_status` | Sync state machine (see stages below) |
| `ton_node_engine_last_mc_block_seqno` | Latest applied masterchain block seqno |
| `ton_node_engine_timediff_seconds` | Seconds between now and last applied MC block |
| `ton_node_engine_shards_timediff_seconds` | Seconds between now and MC block last processed by shard client |

**Sync stages** (`sync_status` values):

| Value | Name | Description |
|-------|------|-------------|
| 0 | `not_set` | Initial state, node just started |
| 1 | `boot` | Downloading init block proof, key blocks |
| 2 | `load_states` | Downloading and applying persistent states (long phase, seqno does not advance) |
| 3 | `finish_boot` | Boot complete, preparing to sync |
| 4 | `sync_archives` | Syncing via archives (bulk download) |
| 5 | `sync_blocks` | Syncing block-by-block from peers |
| 6 | `synced` | Masterchain caught up, shard client within 16 MC blocks |
| 7 | `checking_db` | DB integrity check in progress |
| 8 | `db_broken` | DB corruption detected |

**Terminal conditions:**

| Condition | Trigger | GitHub status | Pod behavior |
|-----------|---------|---------------|--------------|
| Synced | `sync_status = 6` | `success` | Sleeps forever (replaced on next run) |
| DB broken | `sync_status = 8` | `failure` | Sleeps forever (stays for debugging) |
| Timeout | Elapsed > `SYNC_TIMEOUT` (default 24h) | `failure` | Sleeps forever (stays for debugging) |

**Watcher log output:**

```
[watcher] stage=boot seqno=0 mc_timediff=0s shards_timediff=0s elapsed=0h0m
[watcher] stage=boot seqno=47554071 mc_timediff=200000s shards_timediff=200000s elapsed=0h1m
[watcher] stage=load_states seqno=58847563 mc_timediff=67000s shards_timediff=67000s elapsed=0h30m
[watcher] stage=sync_archives seqno=58850000 mc_timediff=10000s shards_timediff=10000s elapsed=1h00m
[watcher] stage=sync_blocks seqno=58870000 mc_timediff=500s shards_timediff=600s elapsed=3h00m
[watcher] stage=synced seqno=58881412 mc_timediff=2s shards_timediff=2s elapsed=3h34m
[watcher] SUCCESS — node synced
```

### Debugging failures

On failure (timeout or DB broken) the pod stays alive. Inspect logs:

```bash
# Watcher log (sync progress)
kubectl logs synctest-mainnet-0 -c watcher -n ton-synctest

# Node log (last 200 lines)
kubectl logs synctest-mainnet-0 -c ton-node -n ton-synctest --tail=200

# Full node log file (written by log4rs)
kubectl exec -it synctest-mainnet-0 -c ton-node -n ton-synctest -- tail -200 /logs/output.log

# Useful greps for node log
kubectl exec synctest-mainnet-0 -c ton-node -n ton-synctest -- grep -e boot -e sync /logs/output.log | tail -20
kubectl exec synctest-mainnet-0 -c ton-node -n ton-synctest -- grep Applied /logs/output.log | tail -20
```

### Re-running

Each workflow run **deletes the previous deployment** (helm uninstall + PVC cleanup) before deploying fresh. The old watcher catches SIGTERM and sets `failure "Cancelled"` on its commit so no commit is left stuck in `pending`.

This means: if a sync test is still running and you trigger a new one, the old test is cancelled and its commit gets a red status. Inspect logs before re-running if you need to debug a failure.

## How to run

```bash
gh workflow run sync-test.yml -R RSquad/ton-node
```

Or: GitHub UI > Actions > Sync Test > Run workflow.

Check current commit status:

```bash
gh api repos/RSquad/ton-node/commits/<sha>/status \
  --jq '.statuses[] | select(.context=="sync-test/mainnet") | {state, description}'
```

## Files

| File | Purpose |
|------|---------|
| `.github/workflows/sync-test.yml` | CI workflow: build image, push to GHCR, helm deploy |
| `ci/sync-test/values.yaml` | Helm values override for the `ton-rust-node` chart |
| `ci/sync-test/watcher.sh` | Sidecar script: poll Prometheus metrics, set GitHub commit status |
| `ci/sync-test/gen-node-config.sh` | Generates node config with random ADNL keys for given IP |
| `ci/sync-test/README.md` | This file |

## Cluster setup from scratch

All commands target cluster `velia-sgp1`. The namespace is `ton-synctest`.

### 1. Create namespace

```bash
kubectl create ns ton-synctest
```

### 2. Image pull secret

Required for pulling node images from GHCR.

```bash
kubectl create secret docker-registry ghcr -n ton-synctest \
  --docker-server=ghcr.io \
  --docker-username=<github-user> \
  --docker-password=<github-pat-with-packages-read>
```

### 3. GitHub token for commit statuses

The watcher needs a token to set commit statuses from inside the K8s pod. Create a fine-grained PAT:

1. Go to https://github.com/settings/tokens?type=beta
2. Repository access: select `RSquad/ton-node`
3. Permissions: **Commit statuses > Read and write** (nothing else)

```bash
kubectl create secret generic credentials -n ton-synctest \
  --from-literal=GITHUB_TOKEN=github_pat_...
```

### 4. Kubeconfig for CI

The GitHub Actions runner needs kubectl/helm access to the cluster. Create a dedicated Rancher user with minimal permissions:

1. **Rancher UI > Users & Authentication > Create**: username `synctest-ci`, global role `User-Base`
2. **Create a Project** in cluster `velia-sgp1` containing namespace `ton-synctest`
3. **Add `synctest-ci` as Project Member** to that project
4. **Download kubeconfig** for `synctest-ci` (login as that user in Rancher UI, or via Rancher API)

Add to GitHub repo secrets (base64-encoded):

```bash
cat kubeconfig.yaml | base64 | gh secret set SYNCTEST_VELIA_SGP1_KUBECONFIG -R RSquad/ton-node
```

### 5. Node IP

The external IP for the ADNL LoadBalancer. Must match an IP available in the MetalLB pool.

```bash
gh secret set SYNCTEST_NODE_IP -R RSquad/ton-node -b "<ip>"
```

CI uses this to generate the node config (ADNL address) and the MetalLB annotation.

### Summary of resources

**Kubernetes (ton-synctest namespace):**

| Resource | Name | Purpose |
|----------|------|---------|
| Namespace | `ton-synctest` | Isolates sync test workloads |
| Secret | `ghcr` | Image pull credentials for GHCR |
| Secret | `credentials` | GitHub PAT for commit status API |
| ConfigMap | `synctest-watcher` | Watcher script (created by CI) |
| Secret | `*-node-configs` | Node config with ADNL keys (created by Helm) |

**GitHub Secrets:**

| Secret | Purpose |
|--------|---------|
| `SYNCTEST_VELIA_SGP1_KUBECONFIG` | Kubeconfig for CI to access cluster |
| `SYNCTEST_NODE_IP` | External IP for ADNL service |

## Configuration reference

### Sync timeout

Environment variable `SYNC_TIMEOUT` in `values.yaml` (seconds). Default: `86400` (24 hours). If the node does not reach `sync_status=6` within this window, the test fails.

### ADNL IP

Set via GitHub Secret `SYNCTEST_NODE_IP`. CI uses it in both the node config (ADNL address) and MetalLB annotation (LoadBalancer IP). They must match.

### Helm chart version

In `.github/workflows/sync-test.yml`, env `HELM_CHART_VERSION`. Must match a published version of `oci://ghcr.io/rsquad/ton-rust-node/helm/node`.

### Resources

Inherited from the Helm chart defaults (8 CPU / 32Gi request, 16 CPU / 64Gi limit). Override in `values.yaml` under `resources` if needed.

### Storage

DB volume uses the chart default (1Ti, `local-path` storage class). All PVCs have `resourcePolicy: ""` — they are deleted together with the Helm release on `helm uninstall`.

### Global config

Uses the mainnet `global.config.json` bundled in the Helm chart. No manual config needed.

### Logs config

Uses the default `logs.config.yml` bundled in the Helm chart. Node logs are written to `/logs/output.log` inside the pod.
