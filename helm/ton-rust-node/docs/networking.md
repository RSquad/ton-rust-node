# Networking

A TON node needs a stable, publicly reachable IP address — other nodes in the network connect to the `adnl_node.ip_address` specified in the node config. This page describes the supported ways to expose the node from Kubernetes.

## Table of contents

- [LoadBalancer with static IP (recommended)](#loadbalancer-with-static-ip-recommended)
- [NodePort](#nodeport)
- [hostNetwork](#hostnetwork)
- [Ingress-nginx stream proxy](#ingress-nginx-stream-proxy)
- [Comparison](#comparison)

## LoadBalancer with static IP (recommended)

The default and simplest approach. Each replica gets its own LoadBalancer Service. The external IP is assigned via provider-specific annotations.

Works with cloud providers (AWS, GCP, Azure) and MetalLB on bare-metal.

```yaml
services:
  type: LoadBalancer
  externalTrafficPolicy: Local
```

### MetalLB

```yaml
services:
  perReplica:
    - annotations:
        metallb.universe.tf/loadBalancerIPs: "192.168.1.100"
    - annotations:
        metallb.universe.tf/loadBalancerIPs: "192.168.1.101"
```

### AWS Elastic IP

```yaml
services:
  perReplica:
    - annotations:
        service.beta.kubernetes.io/aws-load-balancer-eip-allocations: "eipalloc-aaa"
    - annotations:
        service.beta.kubernetes.io/aws-load-balancer-eip-allocations: "eipalloc-bbb"
```

### GCP

```yaml
services:
  perReplica:
    - annotations:
        networking.gke.io/load-balancer-ip-addresses: "my-ip-ref-0"
    - annotations:
        networking.gke.io/load-balancer-ip-addresses: "my-ip-ref-1"
```

The `adnl_node.ip_address` in the node config must match the external IP assigned to that replica's service.

## NodePort

Uses Kubernetes NodePort services instead of a LoadBalancer. Traffic arrives at `<node-ip>:<nodePort>` and is forwarded to the pod.

```yaml
services:
  type: NodePort
  externalTrafficPolicy: Local
```

With `externalTrafficPolicy: Local` the traffic is only routed to pods on the node that received it — no cross-node hops.

### Trade-offs

- No cloud LB or MetalLB required — works on any cluster.
- **You must manage port conflicts yourself.** If you run multiple replicas, each must use different ports (NodePort range is 30000-32767 by default). With a single replica this is not an issue.
- The `adnl_node.ip_address` in the node config must be set to the node's external IP and the NodePort (not the container port).
- You need to ensure the pod is scheduled on the node whose IP you configured — use `nodeSelector` or `nodeAffinity`.

## hostNetwork

The pod binds directly to the host's network interface. No NAT, no Service abstraction — the pod IS the endpoint.

```yaml
hostNetwork: true
```

The node listens on `<host-ip>:<container-port>` directly.

### Trade-offs

- Zero NAT overhead — best network performance.
- **One pod per node per port.** If two pods try to bind the same port on the same host, the second one fails. Use `podAntiAffinity` or `nodeSelector` to spread replicas across nodes.
- The `adnl_node.ip_address` in the node config must match the host's external IP.
- Services are still created but are optional — you can set `services.type: ClusterIP` or leave them as LoadBalancer for in-cluster DNS.

### Example with anti-affinity

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

## Ingress-nginx stream proxy

If you already run ingress-nginx, you can use its TCP/UDP stream proxy to forward traffic to the node pods. No chart changes needed — configure ingress-nginx externally.

The idea: ingress-nginx listens on the external IP and forwards raw TCP/UDP streams to the TON node's ClusterIP services.

### Trade-offs

- Reuses existing infrastructure — no additional LB needed.
- The `adnl_node.ip_address` in the node config must be the ingress controller's external IP.
- Adds a proxy hop (ingress-nginx sits between the client and the node).
- Configuration is external to this chart — you manage the ingress-nginx `tcp-services` / `udp-services` ConfigMaps yourself.

### Example ingress-nginx ConfigMap

```yaml
# TCP services (control, liteserver)
apiVersion: v1
kind: ConfigMap
metadata:
  name: tcp-services
  namespace: ingress-nginx
data:
  "50000": "ton/my-node-0:50000"
  "40000": "ton/my-node-0:40000"
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

| Mode | NAT overhead | LB required | Port management | Complexity |
|------|-------------|-------------|-----------------|------------|
| LoadBalancer + annotations | DNAT | yes (cloud LB / MetalLB) | automatic | low |
| NodePort | kube-proxy | no | manual (one port range per replica) | medium |
| hostNetwork | none | no | manual (one pod per node) | medium |
| Ingress-nginx stream | proxy hop | no (reuses ingress) | manual (ingress ConfigMaps) | medium |

For most deployments, **LoadBalancer with static IP** is the recommended approach. Use hostNetwork when you need zero NAT overhead on bare-metal. Use NodePort or ingress-nginx when you don't have a LoadBalancer controller available.
