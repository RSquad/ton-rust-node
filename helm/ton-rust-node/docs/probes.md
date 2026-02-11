# Health probes

Kubernetes liveness, readiness, and startup probes for the TON node.

## Table of contents

- [Available endpoints](#available-endpoints)
- [Enabling probes](#enabling-probes)
- [Startup probe](#startup-probe)
- [Tuning](#tuning)

## Available endpoints

The TON node metrics HTTP server exposes two health endpoints:

| Endpoint | Purpose | Kubernetes probe |
|----------|---------|-----------------|
| `/healthz` | Liveness check | `livenessProbe` |
| `/readyz` | Readiness check | `readinessProbe` |

Both endpoints return HTTP 200 with a JSON body:

```json
{
  "status": "ok",
  "sync_status": 6,
  "last_mc_block_seqno": 12345678,
  "validation_status": 3
}
```

These endpoints are served by the same HTTP server as `/metrics` and require the `metrics` section in the node config. See [node-config.md](node-config.md).

> **Note:** Both `/healthz` and `/readyz` currently return 200 if the metrics HTTP server is running. They do not check sync status or other internal health criteria. A future version may add sync-aware readiness checks.

## Enabling probes

Probes require `ports.metrics` to be set in the Helm values:

```yaml
ports:
  metrics: 9100

probes:
  startup:
    httpGet:
      path: /healthz
      port: metrics
    failureThreshold: 60
    periodSeconds: 10
  liveness:
    httpGet:
      path: /healthz
      port: metrics
    periodSeconds: 30
    failureThreshold: 3
  readiness:
    httpGet:
      path: /readyz
      port: metrics
    periodSeconds: 10
    failureThreshold: 3
```

The `port: metrics` value references the named container port — it resolves to whatever value `ports.metrics` is set to.

## Startup probe

The startup probe is critical for TON nodes. The node can take several minutes to start, depending on:

- Database size and integrity checks
- State loading and Merkle tree reconstruction
- Network bootstrap and peer discovery

Without a startup probe, the liveness probe would kill the pod before the node finishes starting.

Recommended settings:

| Parameter | Value | Rationale |
|-----------|-------|-----------|
| `failureThreshold` | `60` | Allow up to 10 minutes to start (60 * 10s) |
| `periodSeconds` | `10` | Check every 10 seconds |

Once the startup probe succeeds, Kubernetes switches to the liveness and readiness probes.

## Tuning

### Validators

Validators have stricter uptime requirements. Consider tighter probe settings:

```yaml
probes:
  startup:
    httpGet:
      path: /healthz
      port: metrics
    failureThreshold: 60
    periodSeconds: 10
  liveness:
    httpGet:
      path: /healthz
      port: metrics
    periodSeconds: 15
    failureThreshold: 3
  readiness:
    httpGet:
      path: /readyz
      port: metrics
    periodSeconds: 5
    failureThreshold: 2
```

### Fullnodes / liteservers

Fullnodes are more tolerant of brief interruptions. The default values from the [quick start](#enabling-probes) are appropriate.

### Without the metrics port

If you cannot enable the metrics HTTP server, you can use a TCP socket probe on the control port as a basic liveness check:

```yaml
ports:
  control: 50000

probes:
  liveness:
    tcpSocket:
      port: control
    periodSeconds: 30
    failureThreshold: 3
```

This only verifies that the port is accepting connections — it does not check node health. Use this as a last resort.
