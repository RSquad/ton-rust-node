# Monitoring

Setting up Prometheus and Grafana for TON node metrics. We recommend [kube-prometheus-stack](https://github.com/prometheus-community/helm-charts/tree/main/charts/kube-prometheus-stack) — the chart includes a ServiceMonitor template for automatic scrape discovery.

## Table of contents

- [Prerequisites](#prerequisites)
- [Network security](#network-security)
- [Quick start](#quick-start)
- [ServiceMonitor configuration](#servicemonitor-configuration) (recommended)
- [Alternative: Prometheus annotations](#alternative-prometheus-annotations)
- [Alternative: static scrape config](#alternative-static-scrape-config)
- [Grafana dashboard](#grafana-dashboard)
- [Alert rules](#alert-rules)

## Prerequisites

1. **Enable the metrics HTTP server** in the node config (`config.json`):

```json
{
  "metrics": {
    "address": "0.0.0.0:9100",
    "global_labels": {
      "network": "mainnet",
      "node_id": "my-node-0"
    }
  }
}
```

The server exposes `/metrics` (Prometheus format), `/healthz` (liveness), and `/readyz` (readiness). If the `metrics` section is absent, the server is not started. See [node-config.md](node-config.md) for all options.

> **Note:** `global_labels` with `network` and `node_id` are required for the bundled [Grafana dashboard](../../../grafana/) to work. Without them, dashboard variables will be empty and panels will show no data.

2. **Set `ports.metrics`** in your Helm values:

```yaml
ports:
  metrics: 9100
```

The port number must match the `metrics.address` port in the node config.

## Network security

The metrics port is **never exposed on the public LoadBalancer** per-replica services. A dedicated internal `<release>-metrics` ClusterIP service is created instead — accessible only inside the cluster.

If you need external access to metrics, you can create your own LoadBalancer service pointed at the metrics port. However, the recommended approach is to set up an Ingress with authentication (basic auth, OAuth2 proxy, etc.) that proxies to the `<release>-metrics` ClusterIP service. The chart does not provide external access out of the box — securing an unauthenticated HTTP endpoint is your responsibility.

## Quick start

Minimal values to enable metrics, probes, and ServiceMonitor:

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

metrics:
  serviceMonitor:
    enabled: true
```

## ServiceMonitor configuration

Enable the ServiceMonitor to have kube-prometheus-stack discover and scrape the node automatically:

```yaml
metrics:
  serviceMonitor:
    enabled: true
```

### Label matching

Some Prometheus Operator installations filter ServiceMonitors by labels (the `serviceMonitorSelector` field in the Prometheus CRD). If your Prometheus requires specific labels:

```yaml
metrics:
  serviceMonitor:
    enabled: true
    labels:
      release: kube-prometheus-stack
```

### Scrape interval

By default the ServiceMonitor inherits the global scrape interval from Prometheus (typically 30s). To override:

```yaml
metrics:
  serviceMonitor:
    enabled: true
    interval: "15s"
    scrapeTimeout: "10s"
```

### Cross-namespace monitoring

If Prometheus runs in a different namespace, set the ServiceMonitor namespace to where Prometheus looks:

```yaml
metrics:
  serviceMonitor:
    enabled: true
    namespace: monitoring
```

A `namespaceSelector` is automatically added so Prometheus can discover services in the release namespace.

## Alternative: Prometheus annotations

If you don't use the Prometheus Operator but your Prometheus scrapes services by `prometheus.io/*` annotations:

```yaml
metrics:
  annotations:
    enabled: true
```

This adds `prometheus.io/scrape`, `prometheus.io/port`, and `prometheus.io/path` to the `<release>-metrics` ClusterIP service.

## Alternative: static scrape config

For any other Prometheus setup, the metrics endpoint is available via the internal ClusterIP service. Service DNS: `<release>-metrics.<namespace>.svc.cluster.local`.

## Grafana dashboard

A Grafana dashboard is available in [`grafana/`](../../../grafana/). It is defined as TypeScript (Grafana Foundation SDK) and compiled to JSON.

See [`grafana/README.md`](../../../grafana/README.md) for build and import instructions.

## Alert rules

Example `PrometheusRule` for kube-prometheus-stack. Create this as a separate manifest or include it in your monitoring stack:

```yaml
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: ton-node-alerts
  labels:
    release: kube-prometheus-stack
spec:
  groups:
    - name: ton-node
      rules:
        - alert: TonNodeNotSynced
          expr: ton_node_engine_timediff_seconds > 60
          for: 5m
          labels:
            severity: warning
          annotations:
            summary: "TON node is {{ $value | humanize }}s behind"

        - alert: TonNodeSyncStalled
          expr: ton_node_engine_timediff_seconds > 300
          for: 10m
          labels:
            severity: critical
          annotations:
            summary: "TON node sync stalled, {{ $value | humanize }}s behind"

        - alert: TonNodeValidationFailing
          expr: >
            rate(ton_node_validator_failures_total[5m]) > 0
            and ton_node_validator_status == 3
          for: 5m
          labels:
            severity: warning
          annotations:
            summary: "TON validator reporting failures"

        - alert: TonNodeNotValidating
          expr: >
            ton_node_engine_will_validate == 1
            and ton_node_validator_status < 3
          for: 15m
          labels:
            severity: critical
          annotations:
            summary: "TON node intends to validate but status is {{ $value }}"

        - alert: TonNodeCollationSlow
          expr: >
            histogram_quantile(0.99,
              rate(ton_node_collator_duration_seconds_bucket[5m])
            ) > 2
          for: 10m
          labels:
            severity: warning
          annotations:
            summary: "P99 collation duration > 2s"

        - alert: TonNodeDbQueueBacklog
          expr: ton_node_db_shardstate_queue_size > 10
          for: 5m
          labels:
            severity: warning
          annotations:
            summary: "Shard state processing queue backlog: {{ $value }}"
```

Adjust thresholds and `for` durations to match your SLA requirements. See [metrics.md](metrics.md) for the full metrics reference and PromQL examples.
