# Nodectl Staking Strategies

nodectl picks a stake amount for every election round using a **stake policy**. Each binding resolves its effective policy by checking `elections.policy_overrides` first and falling back to `elections.policy`, so you can mix strategies across nodes in a single config.

There are four strategies, configured via `config elections stake-policy` (or `POST /v1/elections/settings`):

| Strategy | Config value | Amount staked per round |
|---|---|---|
| [Minimum](#minimum) | `"minimum"` | Chain's minimum participation stake |
| [Fixed](#fixed) | `{ "fixed": <nanotons> }` | A constant amount, clamped to `[min_stake, available]` |
| [Split50](#split50) | `"split50"` | Half of available balance, floored at `min_stake` |
| [AdaptiveSplit50](#adaptivesplit50) | `"adaptive_split50"` | Half when competitive, otherwise all — estimates the Elector's selection threshold |

All four are evaluated on every elections tick. If the resulting stake exceeds the free balance, the round is skipped with an error; if the current submission already covers the required stake, no additional message is sent.

> **TONCore nominator caveat.** `Split50` and `AdaptiveSplit50` both assume a single pool can re-split its balance across alternating rounds. The TONCore nominator uses **two separate pools** (even / odd) that each stake in only one round, so splitting inside a round has no effect. When the binding points at a TONCore nominator, the runner ignores these two policies and stakes the **full liquid balance of the selected pool** (still floored at `min_stake`). To stake less, switch the binding (or the per-node override) to `Minimum` or `Fixed`.

### Shared terms

| Term | Description |
|---|---|
| **Elector** | TON smart contract that runs validator elections (`config param 1`). |
| **min_stake** | Minimum stake accepted by the Elector for this round (from `config param 17` / election data). Hard floor for all strategies. |
| **available** | Funds that can be staked: `frozen_stake + free_pool_balance + current_stake`. |
| **current_stake** | Stake already submitted to the active election (0 if none). |

---

## Minimum

Stakes exactly **`min_stake`** — the chain-enforced floor for participating in the round.

```text
stake = min_stake
```

**When to use.** Testing / small-balance setups, or when another process tops up the pool externally and you only want nodectl to keep a seat open with the smallest valid bid.

**Trade-offs.** You always participate with the cheapest possible submission but have essentially no chance of being selected whenever competitors raise their stakes. Expect empty rounds on a busy network.

---

## Fixed

Stakes a constant amount that you control, clamped to the legal range:

```text
stake = clamp(configured_amount, min_stake, available)
```

- If the configured amount is below `min_stake`, nodectl raises it to `min_stake`.
- If it exceeds `available`, nodectl caps it at `available`.

Configured in TON from the CLI (`--fixed <TON>`) and in nanotons via REST (`{"fixed": <nanotons>}`).

**When to use.** You know exactly how much validator capital should be at risk per round and want deterministic accounting — for example, budgeting rewards per wallet or holding a cold reserve off-chain.

**Trade-offs.** The amount does not react to competitors. Under-bid and you never validate; over-bid and you lock up capital that would have been as effective at a lower number.

---

## Split50

Stakes half of the available balance, with `min_stake` as the floor:

```text
stake = max(available / 2, min_stake)
```

The intent is to keep two rounds worth of capital working at once: half goes into the current round while the other half is ready for the next.

**When to use.** Default for most validator setups. Good steady-state behaviour when the balance comfortably exceeds `min_stake` and you want to validate every round.

**Trade-offs.** Split50 does **not** estimate whether half will actually be selected by the Elector. On a crowded round where the effective selection threshold is above `available / 2`, you still stake half and then lose to higher bidders. AdaptiveSplit50 is the competitive-aware upgrade.

**TONCore nominator.** Ignored on a TONCore binding — see the caveat in the intro. The runner stakes the full liquid balance of the selected pool (still ≥ `min_stake`).

---

## AdaptiveSplit50

Like Split50, but estimates the Elector's **minimum effective stake** (the amount actually required to win a seat this round) and falls back to staking the full available balance when half would not clear that bar.

### Overview

AdaptiveSplit50 splits your funds in half and stakes each half into alternating election rounds, so capital is always working. If half is not enough to be selected by the Elector, it stakes everything into the current round instead.

### Key Terms

| Term | Description |
|---|---|
| **min_eff_stake** | Estimated minimum stake the Elector would **select** (not merely accept). Below this — no seat. |
| **frozen_stake** | Stake locked in the previous validation round. |
| **free_pool_balance** | Funds available for staking on the pool balance. |
| **available** | `frozen_stake + free_pool_balance + current_stake` |
| **half** | `available / 2` |
| **sleep_period** | Minimum wait time (fraction of election duration) before acting. |
| **waiting_period** | Maximum wait time for enough participants before using fallback data. Must be ≥ `sleep_period`. |

### How It Works

The strategy runs on every tick (periodic check) during an election.

#### 1. Wait for the right moment

Before doing anything, the strategy waits until:

- The **sleep_period** has passed since the election started, **and**
- At least `min_validators` participants have submitted stakes.

If the **waiting_period** expires and there still aren't enough participants, the strategy stops waiting and proceeds with whatever data is available.

#### 2. Estimate min_eff_stake

The strategy needs to know the minimum stake required to be selected. It uses two sources:

- **Current election estimate** — emulates the Elector's selection algorithm on the participants who have already submitted stakes. Available only when enough participants are present.
- **Previous election data** — takes the smallest frozen stake from the last completed election. Cached per election round.

**Priority:** use the current election estimate when available. Fall back to previous election data only when the current estimate cannot be computed (not enough participants). If neither is available, the strategy skips the election with an error.

#### 3. Decide how much to stake

```text
available = frozen_stake + free_pool_balance + current_stake
half = available / 2
```

- **half >= min_eff_stake** — stake half. The other half is reserved for the next round.
- **half < min_eff_stake** — stake all free funds. Splitting is pointless because the remaining half would also be below the threshold.

**Guards:**

- If `free_pool_balance` is too low to cover the required stake, the strategy skips the election and logs an error.
- If `current_stake` already meets or exceeds `min_eff_stake`, no action is taken.

#### 4. Top up on subsequent ticks

On every tick after the initial submission, the strategy re-evaluates:

- If `min_eff_stake` has risen above `current_stake` (e.g. larger stakes arrived), it tops up by the difference.
- The same half-vs-all logic applies: if the remaining funds can't cover the next round, everything goes into the current one.

### Configuration

The parameters below live under the `elections` section of `nodectl-config.json`.

| Parameter | Type | Default | Description |
|---|---|---|---|
| `sleep_period_pct` | float (0.0–1.0) | `0.2` | Fraction of election duration to wait before acting. |
| `waiting_period_pct` | float (0.0–1.0) | `0.4` | Max fraction of election duration to wait for participants. Must be ≥ `sleep_period_pct`. |

### Decision Flowchart

```text
Election starts
       │
       ▼
  Wait for sleep_period AND min_validators participants
       │
       ├─ Both met ──► Emulate election → curr_min_eff
       │
       └─ Timeout ───► curr_min_eff = None
       │
       ▼
  Fetch prev_min_stake from past elections (cached)
       │
       ▼
  min_eff_stake = curr_min_eff ?? prev_min_stake
       │
       ├─ Neither available ──► Skip election (error)
       │
       ▼
  half = available / 2
       │
       ├─ half >= min_eff_stake ──► Stake half
       │
       └─ half < min_eff_stake  ──► Stake all
       │
       ▼
  ┌─ On every tick: ──────────────────────────────┐
  │                                                │
  │  Re-estimate min_eff_stake                     │
  │                                                │
  │  If min_eff_stake > current_stake → top up     │
  │                                                │
  │  Apply same half-vs-all logic                  │
  └────────────────────────────────────────────────┘
```

**When to use.** Production validators that want Split50's capital efficiency without losing rounds to rising competitor bids.

**Trade-offs.**

- **Needs an estimate of `min_eff_stake`.** The strategy relies on either (a) the Elector emulator running on the current round's participants or (b) the smallest frozen stake from the previous round. If neither is available — e.g. a brand-new chain's first election or a round with too few bidders before `waiting_period_pct` expires — the tick is skipped with an error and no stake is submitted.
- **Delays the first submission.** Unlike `Split50`, AdaptiveSplit50 deliberately waits at least `sleep_period_pct` of the election duration before acting, so it can observe competitor bids. On very short election windows this leaves less time to react if the message fails and has to be retried.
- **Extra on-chain messages for top-ups.** Every tick after the initial submission re-estimates `min_eff_stake`; if it has risen, the runner sends an additional stake message for the delta. Each top-up costs gas (`ELECTOR_STAKE_FEE + wallet/pool compute fees`). On a volatile round you may pay for several top-ups.
- **Estimate can lag reality.** The emulator only sees stakes that have already been submitted. A late burst of large bids after your final submission can still push `min_eff_stake` above your accepted stake — AdaptiveSplit50 will chase on the next tick, but only while the round is still open.
- **Requires `sleep_period_pct` / `waiting_period_pct` tuning** for networks with unusual election pacing. The defaults (0.2 / 0.4) assume standard TON mainnet cadence.

**TONCore nominator.** Ignored on a TONCore binding — see the caveat in the intro. The adaptive emulator / top-up path is skipped and the runner stakes the full liquid balance of the selected pool (still ≥ `min_stake`).

---

## Choosing a strategy

- **Default** — `Split50` is a reasonable starting point on a network where your balance is comfortably above `min_stake` and competitor bids are stable.
- **Competitive network** — switch to `AdaptiveSplit50` once you have enough balance that "half" would matter; it reacts to live Elector pressure.
- **Budgeted capital** — `Fixed` when you want predictable exposure per round regardless of free balance.
- **Smoke tests / CI** — `Minimum` keeps bids at the legal floor.
- **TONCore nominator binding** — use `Fixed` or `Minimum` if you need to cap per-round exposure. `Split50` and `AdaptiveSplit50` are accepted but collapse to "stake the full liquid balance of the selected pool" because the two TONCore pools can't both stake in the same round.

Policies can be mixed per node via `policy_overrides`:

```bash
# Default for all nodes
nodectl config elections stake-policy --adaptive-split50

# But keep node0 on a conservative fixed bid
nodectl config elections stake-policy --node node0 --fixed 10000
```
