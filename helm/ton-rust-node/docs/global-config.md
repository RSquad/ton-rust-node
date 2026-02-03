# Global config (global.config.json)

The global config describes the TON network the node should connect to: DHT bootstrap nodes, public liteservers, and validator init/zero state references. It is the same for all nodes on a given network.

A mainnet default is bundled with the chart at [files/global.config.json](../files/global.config.json) and used automatically if no override is given. The bundled config is taken from the official source at [ton-blockchain.github.io](https://ton-blockchain.github.io/).

## Table of contents

- [When to override](#when-to-override)
- [How to override](#how-to-override)
- [Structure overview](#structure-overview)

## When to override

For most deployments you don't need to touch the global config at all. You only need to provide your own if:

- You want to use a **testnet** instead of mainnet — download the testnet config from the official source.
- You run a **private network** with custom DHT nodes and overlays.
- The bundled config is **outdated** — the official config may change over time (new DHT nodes, new init blocks). It is recommended to fetch a fresh copy periodically.

## How to override

```bash
# Download the latest mainnet config
curl -o global.config.json https://ton-blockchain.github.io/global.config.json

# Pass it to the chart
helm install my-node ./helm/ton-rust-node \
  --set-file globalConfig=./global.config.json \
  ...
```

Or inline in values:

```yaml
globalConfig: |
  {"@type": "config.global", "dht": {...}, ...}
```

Or reference an existing ConfigMap:

```yaml
existingGlobalConfigMapName: my-global-config
```

## Structure overview

The config has three top-level sections:

| Section | Purpose |
|---------|---------|
| `dht` | DHT bootstrap nodes — the initial peers the node contacts to discover the network |
| `liteservers` | Public liteserver endpoints (used by lite clients, not by the node itself) |
| `validator` | Network identity: zero state hash, init block, and hardfork references |

No fields in the global config need to match Helm values — it is passed through to the node as-is.

For a full description of the config format, see the [official TON documentation](https://ton-blockchain.github.io/).
