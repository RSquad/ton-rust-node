# TON Blockchain Scripts

A collection of scripts for testing TON blockchain

## Prerequisites

- [Bun](https://bun.sh/) runtime installed
- A valid `.env` file with required configuration

## Configuration

Follow these steps to set up the project:

1. **Install dependencies**
   ```bash
   bun install
   ```

2. **Configure environment**
   
Place your `.env` file in the project root directory. Ensure it contains all necessary environment variables for TON blockchain interaction.

- NETWORK – Network name (e.g., mainnet or testnet).
- WORKCHAIN – Workchain ID (0 = basechain, -1 = masterchain).
- WALLET_ID – Wallet ID (e.g., 42 for rustnet).
- MASTER_WALLET_VERSION – Master wallet version (e.g., V3R2).
- MASTER_WALLET_KEY – Master wallet secret key (encoding: hex).
- FAUCET_WALLET_VERSION – Faucet wallet version (e.g., V3R2).
- FAUCET_WALLET_MNEMONIC – Faucet wallet mnemonic.
- JETTON_MINTER – Jetton minter address.
- API_ENDPOINTS – TonCenter API endpoints separated by |.
- API_BATCH_SIZE – Maximum concurrent API requests per batch.

## Usage

### Transfer Jettons (sequential V1)

Deploy new wallets up to the specified count, and transfer TONs and jettons.
This script creates or appends to ${NETWORK}_wallets.csv with newly generated wallet data (the NETWORK value is taken from .env).
These wallets can be used for concurrent transfer tests.

```bash
bun run ./scripts/transferJettonSeq.ts \
  --count 5 \
  --tons 5 \
  --jettons 50 \
  --stat_level 1 \
  --filter_out_session_changes 1
```

**Parameters:**
- `--count` <int>: Number of recipients (transfers) to process sequentially
- `--tons` <number>: TON to send to each recipient
- `--jettons` <number>: Jettons to send to each recipient
- `--stat_level` <0|1>: 0 – only perform transfers. 1 – emit detailed per-tx statistics and a summary
- `--filter_out_session_changes` <0|1>: 1 - skip transactions that would landed during a catchain session change, 0 - keep all transactions

### Transfer Jettons (sequential V2)

It uses existing wallets from ${NETWORK}_wallets.csv and makes sequential transfers of jettons.

```bash
bun run ./scripts/transferJettonSeq2.ts \
  --count 5 \
  --jettons 0.1
```

**Parameters:**
- `--count` <int>: Number of recipients (transfers) to process sequentially
- `--jettons` <number>: Jettons to send to each recipient

### Transfer Jettons (concurrent)

Transfer jettons to multiple addresses up to the specified count.
Use ${NETWORK}_wallets.csv with wallet data (the NETWORK value is taken from .env).

```bash
bun run ./scripts/transferJettonConcurrent.ts \
  --count 5 \
  --jettons 0.01
```

**Parameters:**
- `--count` <int>: Number of recipients (transfers) to process sequentially
- `--jettons` <number>: Jettons to send to each recipient

### Get Transaction Chain

Retrieve a transaction chain for a specific address, logical time, and hash:

```bash
bun run ./scripts/getTransactionChain.ts \
  --addr="EQDNRGAhJs2GAeiXagdj5XRlmXkU0RP_OQmwbHkt5w4eJ4Xz" \
  --lt=1184483000001 \
  --hash="dd70eef6880464faf3a17a2abf24478aa39f955bce05ba698f691812aa342486" \
  --filter_out_session_changes 1
```

**Parameters:**
- `--addr` <string>: TON wallet address.
- `--lt` <number>: Logical time of the transaction.
- `--hash` <string>: Transaction hash (encoding: hex).
- `--filter_out_session_changes` <0|1>: 1 - skip transactions that would landed during a catchain session change, 0 - keep all transactions
