# Networking

A TON node needs a stable, publicly reachable IP address — other nodes connect to the `adnl_node.ip_address` in the node config. This page covers the chart's networking features and how to choose between them.

## Table of contents

- [Ports and services](#ports-and-services)
- [Exposure modes](#exposure-modes)
  - [LoadBalancer (recommended)](#loadbalancer-recommended)
  - [NodePort](#nodeport)
  - [hostPort](#hostport)
  - [hostNetwork](#hostnetwork)
  - [Ingress-nginx stream proxy](#ingress-nginx-stream-proxy)
- [Comparison](#comparison)
- [NetworkPolicy](#networkpolicy)

## Ports and services

The chart manages five ports. Each port is optional (set to `null` to disable) except ADNL which is always enabled.

| Port | Protocol | Default | Purpose |
|------|----------|---------|---------|
| `ports.adnl` | UDP | `30303` | Peer-to-peer protocol. Must be publicly reachable. |
| `ports.control` | TCP | `50000` | Node management (stop, restart, elections). Must not be public. |
| `ports.liteserver` | TCP | `null` | Liteserver API for external consumers. |
| `ports.jsonRpc` | TCP | `null` | JSON-RPC API for external consumers. |
| `ports.metrics` | TCP | `null` | Prometheus metrics, health and readiness probes. |

### Per-port services

Each enabled port gets its own per-replica Kubernetes Service. This allows independent configuration of service type, annotations, and traffic policy per port.

| Port | Service name | Default type | Rationale |
|------|-------------|-------------|-----------|
| ADNL | `<release>-<i>` | LoadBalancer | Must be publicly reachable for P2P |
| control | `<release>-<i>-control` | ClusterIP | Node management — recommended to keep internal |
| liteserver | `<release>-<i>-liteserver` | LoadBalancer | Serves external API consumers |
| jsonRpc | `<release>-<i>-jsonrpc` | LoadBalancer | Serves external API consumers |
| metrics | `<release>-metrics` | ClusterIP | Internal scraping only. Not per-replica — separate template. |

Override the type per port:

```yaml
services:
  adnl:
    type: LoadBalancer           # default
    externalTrafficPolicy: Local
  control:
    type: ClusterIP              # default — recommended to keep internal
  liteserver:
    type: LoadBalancer           # default
  jsonRpc:
    type: LoadBalancer           # default
```

Each port's service supports `type`, `externalTrafficPolicy`, `annotations`, and `perReplica` overrides. See [LoadBalancer](#loadbalancer-recommended) for `perReplica` examples.

## Exposure modes

The chart supports four ways to make ports reachable from outside the cluster. They control **how traffic reaches the pod**, not which ports are enabled. Choose one mode for your deployment — they can be combined but typically are not.

### LoadBalancer (recommended)

Each per-replica Service gets a cloud load balancer (or MetalLB VIP). The external IP is assigned via provider-specific annotations on the ADNL service.

```yaml
services:
  adnl:
    type: LoadBalancer
    externalTrafficPolicy: Local
```

This is the default — no changes needed for a basic deployment.

#### Static IP assignment

Use `perReplica` annotations to pin IPs. List index matches replica index.

**MetalLB:**

```yaml
services:
  adnl:
    perReplica:
      - annotations:
          metallb.universe.tf/loadBalancerIPs: "192.168.1.100"
      - annotations:
          metallb.universe.tf/loadBalancerIPs: "192.168.1.101"
```

**AWS Elastic IP:**

```yaml
services:
  adnl:
    perReplica:
      - annotations:
          service.beta.kubernetes.io/aws-load-balancer-eip-allocations: "eipalloc-aaa"
      - annotations:
          service.beta.kubernetes.io/aws-load-balancer-eip-allocations: "eipalloc-bbb"
```

**GCP:**

```yaml
services:
  adnl:
    perReplica:
      - annotations:
          networking.gke.io/load-balancer-ip-addresses: "my-ip-ref-0"
      - annotations:
          networking.gke.io/load-balancer-ip-addresses: "my-ip-ref-1"
```

The `adnl_node.ip_address` in the node config must match the external IP assigned to that replica's ADNL service.

---

### NodePort

Uses the Kubernetes NodePort mechanism. Traffic arrives at `<node-ip>:<nodePort>` and is forwarded to the pod.

```yaml
services:
  adnl:
    type: NodePort
    externalTrafficPolicy: Local
```

**When to use:** clusters without a LoadBalancer controller (no cloud LB, no MetalLB).

**Trade-offs:**

- Works on any cluster — no LB infrastructure needed.
- You must manage port conflicts manually. Multiple replicas need different ports (NodePort range is 30000-32767 by default).
- The `adnl_node.ip_address` must be the node's external IP with the NodePort, not the container port.
- The pod must be scheduled on the node whose IP you configured — use `nodeSelector` or `nodeAffinity`.

---

### hostPort

Binds specific container ports directly to the host's network interface. The pod stays in the pod network — only the selected ports are exposed on the host IP. Network policies continue to work.

Each port can be independently enabled:

```yaml
hostPort:
  adnl: true
  control: false     # never expose control on host
  liteserver: false
  jsonRpc: false
```

**When to use:** you need ADNL on the host IP without a LoadBalancer, but want to keep other ports isolated in the pod network. Common on bare-metal with direct public IPs on nodes.

**Trade-offs:**

- Only the enabled ports are exposed on the host — others stay in the pod network behind Services.
- Network policies still work (unlike `hostNetwork`).
- **One pod per node.** The port binds to `0.0.0.0` on the host — two pods on the same node would conflict. Use `podAntiAffinity` or `nodeSelector` to spread replicas.
- The `adnl_node.ip_address` must match the host's external IP.

**Example with anti-affinity:**

```yaml
hostPort:
  adnl: true

affinity:
  podAntiAffinity:
    requiredDuringSchedulingIgnoredDuringExecution:
      - labelSelector:
          matchLabels:
            app.kubernetes.io/name: ton-rust-node
        topologyKey: kubernetes.io/hostname
```

---

### hostNetwork

The pod uses the host's network stack directly. All container ports bind on the host IP. No NAT, no Service abstraction needed — the pod IS the endpoint.

```yaml
hostNetwork: true
```

**When to use:** bare-metal deployments where you need zero NAT overhead and accept the security trade-off.

**Trade-offs:**

- Zero NAT overhead — best possible network performance.
- **All ports are exposed on the host**, including control. Use `networkPolicy` or firewall rules to restrict access.
- **Network policies do not work** — the pod is in the host network namespace.
- **One pod per node.** Same constraint as `hostPort`.
- The `adnl_node.ip_address` must match the host's external IP.
- Services are still created. Set `services.adnl.type: ClusterIP` if you don't need LoadBalancer.

**Example with anti-affinity:**

```yaml
hostNetwork: true

affinity:
  podAntiAffinity:
    requiredDuringSchedulingIgnoredDuringExecution:
      - labelSelector:
          matchLabels:
            app.kubernetes.io/name: ton-rust-node
        topologyKey: kubernetes.io/hostname
```

---

### Ingress-nginx stream proxy

Reuses an existing ingress-nginx controller to forward raw TCP/UDP streams to the node's ClusterIP services. No chart changes needed — configuration is external.

Override service types to ClusterIP for the ports you route through ingress:

```yaml
services:
  liteserver:
    type: ClusterIP
  jsonRpc:
    type: ClusterIP
```

ADNL still needs external reachability — keep it as LoadBalancer or use `hostPort.adnl: true`.

**When to use:** you already run ingress-nginx and don't want additional LoadBalancers for liteserver/jsonRpc. ADNL still needs a direct path (LoadBalancer or hostPort).

**Trade-offs:**

- Reuses existing infrastructure — no additional LB cost.
- Adds a proxy hop (ingress-nginx sits between client and node).
- The `adnl_node.ip_address` must be the ingress controller's external IP.
- Configuration is external — you manage the ingress-nginx `tcp-services` / `udp-services` ConfigMaps.

**Example ingress-nginx ConfigMap:**

```yaml
# TCP services (control, liteserver)
apiVersion: v1
kind: ConfigMap
metadata:
  name: tcp-services
  namespace: ingress-nginx
data:
  "50000": "ton/my-node-0-control:50000"
  "40000": "ton/my-node-0-liteserver:40000"
---
# UDP services (ADNL)
apiVersion: v1
kind: ConfigMap
metadata:
  name: udp-services
  namespace: ingress-nginx
data:
  "30303": "ton/my-node-0:30303"
```

## Comparison

| Mode | NAT overhead | LB required | Port management | Network policies | Complexity |
|------|-------------|-------------|-----------------|-----------------|------------|
| LoadBalancer | DNAT | yes (cloud LB / MetalLB) | automatic | yes | low |
| NodePort | kube-proxy | no | manual (port ranges) | yes | medium |
| hostPort | minimal | no | manual (one pod per node) | yes | medium |
| hostNetwork | none | no | manual (one pod per node) | **no** | medium |
| Ingress-nginx stream | proxy hop | no (reuses ingress) | manual (ConfigMaps) | yes | medium |

**Recommended:** LoadBalancer with static IP for most deployments. Use `hostPort` for bare-metal with direct public IPs when you don't have MetalLB. Use `hostNetwork` only when zero NAT overhead is critical and you accept the security trade-off of exposing all ports.

## NetworkPolicy

The chart can create a NetworkPolicy that allows inbound ADNL UDP from the internet and restricts TCP ports to specified CIDRs.

```yaml
networkPolicy:
  enabled: true
  allowCIDRs:
    - 10.0.0.0/8
```

When `networkPolicy.enabled` is `true`:

- **ADNL (UDP)** is always allowed from `0.0.0.0/0` — required for peer-to-peer connectivity.
- **TCP ports** (control, liteserver, jsonRpc, metrics) get a single ingress rule. If `allowCIDRs` is set, only those CIDRs are allowed. If empty, traffic is not restricted by source.
- **extraIngress** allows appending arbitrary raw ingress rules.

> **Note:** This policy only covers ingress. If your cluster enforces egress policies, allow outbound UDP to `0.0.0.0/0` for ADNL separately.

> **Note:** Network policies have no effect when `hostNetwork: true` — the pod is in the host network namespace.
