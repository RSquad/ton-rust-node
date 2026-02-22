# Elections

> **Alpha software.** nodectl is under active development. Configuration format, CLI interface, and Helm chart values may change between releases without notice.

How nodectl participates in TON validator elections.

## Table of contents

- [How elections work](#how-elections-work)
- [Stake policies](#stake-policies)
- [Per-node overrides](#per-node-overrides)
- [Single Nominator Pool](#single-nominator-pool)
- [Auto-deploy](#auto-deploy)
- [Fee constants](#fee-constants)
- [Managing elections](#managing-elections)
- [Binding status](#binding-status)

---

## How elections work

When the `elections` section is present in the config, nodectl runs a background task that checks for active elections on a configurable interval (default: 40 seconds).

> **Note:** Nodes do not participate in elections by default. You must explicitly enable each node with `nodectl config elections enable <binding> ...` before it will submit stakes.

### Algorithm

1. **Check for active elections** — query `get_active_election_id` on the Elector contract. If ID is 0, no elections are active — wait for the next tick.

2. **Get election parameters** — read blockchain config parameters #15 (election timing) and #34 (current validators). Query `past_elections` from the Elector contract.

3. **For each managed node** (from bindings):
   - **Recover frozen stake** — check if a previous stake is available for recovery and request it
   - **Calculate stake** — determine the stake amount based on the configured [stake policy](#stake-policies)
   - **Generate validator key** — request the TON node to generate a new Ed25519 validator key via the Control Server (if none exists for this election). Validator keys are ephemeral and auto-managed by the node — you do not need to create them
   - **Form election bid** — prepare the election bid message using the validator key
   - **Submit stake** — send the transaction through the validator wallet or [nomination pool](#single-nominator-pool)

The task repeats every `tick_interval` seconds (default: 40).

> **Note:** Validator keys are distinct from the Control Server client keys and wallet keys in the nodectl config. Client keys authenticate nodectl to the node's Control Server. Wallet keys sign blockchain transactions. Validator keys are used by the node for block signing during the validation round — they are ephemeral (one per election) and auto-managed.

---

## Stake policies

Three policies determine how much TON to stake. All policies fail with an error if the available balance is below the Elector's minimum stake.

### Split50 (default)

Stakes half of the available balance, but never less than the minimum stake:

```
stake = max(available / 2, min_stake)
```

Keeps roughly half the balance liquid for the next election round or unexpected expenses.

Config value: `"split50"`

### Fixed

Stakes a fixed amount in nanoTON, clamped to the valid range:

```
stake = clamp(amount, min_stake, available)
```

If the specified amount is below `min_stake`, it is raised to `min_stake`. If it exceeds the available balance, it is lowered to the available balance.

Config value: `{"fixed": <nanoTON>}` (e.g. `{"fixed": 1000000000000}` = 1000 TON)

### Minimum

Stakes the minimum amount required by the Elector contract:

```
stake = min_stake
```

Conservative approach — deposits the bare minimum to participate. Useful for testing or when you want to keep most funds liquid.

Config value: `"minimum"`

### Setting the policy

Via CLI:

```bash
nodectl config stake-policy --split50
nodectl config stake-policy --fixed 1000000000000
nodectl config stake-policy --minimum
```

### Balance calculation

The available stake is calculated as:

```
available = pool_balance + frozen_stake - reserved
```

Where reserved includes the elector stake fee (1 TON), wallet compute fee (0.2 TON), and a storage reserve (1 TON).

---

## Per-node overrides

Different nodes can use different stake policies. Per-node overrides take precedence over the default:

```json
{
  "elections": {
    "policy": "split50",
    "policy_overrides": {
      "node0": { "fixed": 500000000000 },
      "node1": "minimum"
    }
  }
}
```

Via CLI:

```bash
nodectl config stake-policy --fixed 500000000000 --node node0
nodectl config stake-policy --minimum --node node1

# Reset a per-node override (falls back to default):
nodectl config stake-policy --reset --node node0
```

---

## Single Nominator Pool

nodectl supports Single Nominator Pool (SNP) contracts for staking. When a pool is configured for a node (via bindings), transactions are sent to the pool contract instead of directly to the Elector.

### Configuration

Pools are defined in the top-level `pools` section and connected to nodes via `bindings`:

```json
{
  "pools": {
    "pool0": {
      "kind": "snp",
      "owner": "-1:<OWNER_ADDRESS>"
    }
  },
  "bindings": {
    "node0": {
      "wallet": "wallet0",
      "pool": "pool0"
    }
  }
}
```

### Address computation

The SNP contract address is deterministic:

```
address = hash(snp_code + owner_address + validator_wallet_address)
```

When you specify only the `owner` (no `address`), nodectl computes the pool address on startup using the owner and the validator wallet from the binding.

### Why one wallet per node

Because the SNP address depends on both the owner and the validator wallet address, nodes that share a wallet produce the same pool address. This prevents them from participating in elections independently. **Always use one wallet per node** when using SNP.

---

## Auto-deploy

The `contracts_task` runs alongside the elections task and automatically deploys and maintains contracts using the **master wallet**:

| Step | Action | Cost |
|------|--------|------|
| 1 | Deploy master wallet | balance >= 1 TON |
| 2 | Deploy each validator wallet | 1 TON + 0.1 TON gas per wallet |
| 3 | Deploy each SNP pool | 1 TON + 0.1 TON gas per pool |
| 4 | Top up wallets below 5 TON | 10 TON per top-up |

The master wallet key is auto-generated in vault on first start. You only need to fund the master wallet address — deployment is automatic.

Once all contracts are deployed (`all contracts are ready` in logs), the elections task begins participating.

---

## Fee constants

| Fee | Amount | Description |
|-----|--------|-------------|
| Elector stake fee | 1 TON | Fee for submitting a stake to the Elector |
| Recover fee | 0.2 TON | Fee for recovering a frozen stake |
| Pool compute fee | 0.2 TON | Fee for nominator pool transactions |
| Wallet compute fee | 0.2 TON | Fee for wallet transactions |
| Storage reserve | 1 TON | Minimum balance reserved for storage |

---

## Managing elections

### Enable or disable nodes

Nodes do not participate in elections by default. Enable nodes explicitly:

```bash
kubectl exec deploy/my-nodectl -- nodectl config elections enable node0 node1
```

Disable nodes to stop them from participating:

```bash
kubectl exec deploy/my-nodectl -- nodectl config elections disable node0
```

The service picks up config changes automatically — no restart needed.

### View binding status

```bash
kubectl exec deploy/my-nodectl -- nodectl config elections show
```

### Change stake policy at runtime

Stake policy changes are also made via config. The service applies them on the next tick:

```bash
kubectl exec deploy/my-nodectl -- nodectl config stake-policy --minimum
kubectl exec deploy/my-nodectl -- nodectl config stake-policy --node node0 --fixed 500000000000
```

## Binding status

The service tracks the lifecycle of each binding and writes the status back to the config. Statuses:

| Status | Description |
|--------|-------------|
| `idle` | Node is not participating in elections |
| `participating` | Node is enabled and actively submitting stakes |
| `validating` | Node won the election and is currently validating |
| `draining` | Validation round ended, stake is being recovered |

Transitions happen automatically as the service monitors election rounds and validation periods. Use `nodectl config elections show` to see the current status of all bindings.
