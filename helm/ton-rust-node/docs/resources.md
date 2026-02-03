# Resource recommendations

CPU and memory recommendations for the TON node. These are set via `resources` in Helm values.

| Role | CPU request | CPU limit | Memory request | Memory limit |
|------|-------------|-----------|----------------|--------------|
| Fullnode / Liteserver | 8 | 16 | 32Gi | 64Gi |
| Validator | 16 | 32 | 64Gi | 128Gi |

Fullnode / liteserver:

```yaml
resources:
  requests:
    cpu: "8"
    memory: 32Gi
  limits:
    cpu: "16"
    memory: 64Gi
```

Validator:

```yaml
resources:
  requests:
    cpu: "16"
    memory: 64Gi
  limits:
    cpu: "32"
    memory: 128Gi
```

The node can run on fewer resources, but these values provide headroom for load spikes — elections, heavy traffic, catch-up after restarts, etc.
