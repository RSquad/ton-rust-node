# TON TPS Counter

Measures real transactions per second across all shards on the TON blockchain by scanning a range of masterchain blocks via a liteserver.

## How it works

1. Connects to a TON liteserver via the lite-client protocol
2. Iterates masterchain blocks in the given seqno range
3. For each MC block, discovers all shard blocks and counts transactions (with deduplication)
4. Computes TPS by dividing total transactions by the wall-clock time between the first and last masterchain block

Both masterchain and workchain shard transactions are counted. Per-shard statistics are printed at the end.

## Setup

Install dependencies:

```bash
bun install
```

Create a `.env` file with your liteserver connection details:

```env
LITESERVER_HOST=<lite-server ip>
LITESERVER_PORT=<lite-server port>
LITESERVER_PUBLIC_KEY=<lite-server public key in hex>
NETWORK_NAME="TON_TESTNET"
```

## Usage

```bash
bun run ./scripts/tps.ts -s <start_seqno> -e <end_seqno> [-p <page_size>]
```

### Options

```
-s, --start <seqno>  start masterchain seqno (required)
-e, --end <seqno>    end masterchain seqno (required)
-p, --page <size>    transaction page size (default: 1024)
-h, --help           display help for command
```

### Examples

```bash
# scan 11 masterchain blocks
bun run ./scripts/tps.ts -s 1000 -e 1010

# scan with a smaller page size
bun run ./scripts/tps.ts -s 1000 -e 1010 -p 512
```

## Notes

- The time window is measured from `START_SEQNO - 1` to `END_SEQNO` to correctly cover the period in which counted transactions occurred
- Shard blocks referenced by multiple consecutive MC blocks are deduplicated to avoid double-counting
- If MC blocks fail to load, they are skipped and a warning is shown — TPS may be understated
- Shard lookups within each MC block are parallelized for performance