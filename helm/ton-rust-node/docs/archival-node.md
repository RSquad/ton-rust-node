# Archival node

An archival node keeps the full blockchain history and serves historical queries via liteserver. Setting one up involves two steps: importing existing archives into epoch-based storage, then starting the node in archival mode.

## Table of contents

- [System requirements](#system-requirements)
- [Source data](#source-data)
- [Archive import](#archive-import)
  - [Parameters](#parameters)
  - [What the import does](#what-the-import-does)
  - [Output structure](#output-structure)
- [Archival mode config](#archival-mode-config)
  - [Simple setup](#simple-setup)
  - [Distributed setup](#distributed-setup)
- [Cloning an existing archival node](#cloning-an-existing-archival-node)
- [Helm integration](#helm-integration)

## System requirements

| Resource | Minimum |
|----------|---------|
| Disk | 20 TB (block archives + cells database) |
| RAM | 32 GB |
| Import time | Several days for full blockchain history |

## Source data

TON block archives are stored as pairs of `.pack` files per archive group (masterchain + shard blocks):

```
archive.00000.pack                      # masterchain blocks 0-99
archive.00000.0:8000000000000000.pack   # workchain 0 shard blocks for the same range
archive.00100.pack                      # masterchain blocks 100-199
archive.00100.0:8000000000000000.pack   # workchain 0 shard blocks
...
```

Archives can be obtained using TON storage clients (we used [this](https://github.com/xssnick/tonutils-storage) one). Archives are split into 4 Gb pieces and addressed by bags hash in terms of TON storage. Bags are listed [here](https://archival-dump.ton.org/index/mainnet.json). All of them should be added to TON storage client for download. After download is finished all *.pack files are to be moved into one folder replacing `_` symbol in name with `:` (files are renamed to remove symbols which are not supported on some platforms, but TON archives use `:` in shard archives name). The folder containing all `*.pack` files is the input for `archive_import` util.

You also need:

- Masterchain zerostate `.boc` file. TON mainnet zerostate [here](https://github.com/RSquad/ton-rust-node/blob/master/src/node/src/tests/static/5E994FCF4D425C0A6CE6A792594B7173205F740A39CD56F537DEFD28B48A0F6E.boc)
- Workchain zerostate(s) `.boc` file(s) (one per workchain). TON mainnet 0 workchain zerostate [here](https://github.com/RSquad/ton-rust-node/blob/master/src/node/src/tests/static/EE0BEDFE4B32761FB35E9E1D8818EA720CAD1A0E7B4D2ED673C488E72E910342.boc)
- Global config JSON (contains zerostate hashes and hard fork list). TON mainnet global config [here](https://ton-blockchain.github.io/global.config.json)

## Archive import

The `archive_import` tool converts raw `.pack` files into epoch-based storage used by the archival node.

```bash
RUST_LOG=info archive_import \
  --archives-path /path/to/archives \
  --epochs-path /data/epochs \
  --node-db-path /data/node_db \
  --mc-zerostate /path/to/mc_zerostate.boc \
  --wc-zerostate /path/to/wc0_zerostate.boc \
  --global-config /path/to/global-config.json
```

### Parameters

| Parameter | Required | Description |
|-----------|----------|-------------|
| `--archives-path` | yes | Directory containing source `.pack` files |
| `--epochs-path` | yes | Directory where epoch subdirectories will be created |
| `--node-db-path` | yes | Path to node database |
| `--mc-zerostate` | yes | Path to masterchain zerostate `.boc` file |
| `--wc-zerostate` | yes | Path to workchain zerostate `.boc` file (repeat for each workchain) |
| `--global-config` | yes | Path to global config JSON |
| `--epoch-size` | no | MC blocks per epoch, default `10000000` (must be a multiple of 20000) |
| `--copy` | no | Copy `.pack` files instead of moving them |

### What the import does

1. Validates zerostate hashes against the global config
2. Scans the archives directory and groups `.pack` files by archive ID
3. For each group: deserializes blocks, validates proofs, imports packages into epoch storage
4. Populates the node database: block handles, prev/next block links, block index, shard states

The import supports resume — if interrupted, re-run with the same parameters to continue from the last imported group.

### Output structure

```
/data/node_db/
  db/                  # main RocksDB (block handles, indexes, state keys)
  archive_states/      # shard state cell storage

/data/epochs/
  epoch_0/             # archive packages for MC blocks 0..epoch_size-1
    archive_db/        # RocksDB with package metadata
    archive/packages/  # .pack files
  epoch_1/
    ...
```

---

## Archival mode config

Add the `archival_mode` section to the node config to enable epoch-based archival storage. Set `internal_db_path` to point to the database created by the import.

### Simple setup

All epochs in one directory. The node auto-discovers existing epochs on startup and creates new ones in the same location.

```json
{
  "internal_db_path": "/data/node_db",
  "archival_mode": {
    "epoch_size": 10000000,
    "new_epochs_path": "/data/epochs",
    "existing_epochs": []
  }
}
```

The `epoch_size` must match the value used during import.

### Distributed setup

Imported epochs on separate (slower) storage, new epochs on fast storage. List imported epoch directories explicitly in `existing_epochs`.

```json
{
  "internal_db_path": "/data/node_db",
  "archival_mode": {
    "epoch_size": 10000000,
    "new_epochs_path": "/fast_ssd/new_epochs",
    "existing_epochs": [
      { "path": "/nfs/imported/epoch_0" },
      { "path": "/nfs/imported/epoch_1" },
      { "path": "/fast_ssd/imported/epoch_2" }
    ]
  }
}
```

> **Note:** The last imported epoch is likely incomplete — its range covers blocks still being created. It will continue to receive new blocks during sync, so place it on fast storage alongside `new_epochs_path`.

### Behavior

When `archival_mode` is set:

- Archive GC is disabled — all historical data is preserved
- Shard states are stored in a separate cell database (`archive_states/`)
- New blocks arriving via sync are appended to the latest epoch

> **See also:** For a simpler setup that keeps full history without epoch-based storage, see the [archival node section in node-config.md](node-config.md#archival-node). That approach disables GC and works without the import step, but requires the node to sync all history from scratch.

---

## Cloning an existing archival node

Instead of importing from scratch, you can copy data from a running archival node:

1. Stop the source node
2. Copy epoch directories and the node database (`rsync` or similar)
3. On the new machine, generate a fresh node config with new ADNL keys
4. Set `internal_db_path` and `archival_mode` pointing to the copied data
5. Start the new node

This works because the database contains only blockchain data (blocks, states, indexes). Node identity (ADNL keys, validator keys) is stored in the config file and secrets vault, not in the database.

> **Important:** Do not copy the node config file — it contains the ADNL private keys of the source node. Always generate fresh keys for the new node.

---

## Helm integration

The archive import runs outside of Kubernetes as a one-time migration step. After import, configure the Helm chart to start the node in archival mode:

1. Mount the epoch storage and node database into the pod using `extraVolumes` and `extraVolumeMounts`
2. Set `archival_mode` in the node config (`nodeConfigs`) with paths matching the mount points
3. Size `storage.db.size` for the node database (the epoch data lives on external volumes)

> **See also:** [node-config.md](node-config.md#archival-node) covers the GC-based approach to keeping full history, which does not require the import step but uses more disk on the primary volume.
