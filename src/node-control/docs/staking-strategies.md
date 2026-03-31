# Nodectl Staking Strategies

## AdaptiveSplit50

### Overview

AdaptiveSplit50 splits your funds in half and stakes each half into alternating election rounds, so capital is always working. If half is not enough to be selected by the Elector, it stakes everything into the current round instead.

---

### Key Terms

| Term | Description |
|---|---|
| **Elector** | TON smart contract that runs validator elections. |
| **min_eff_stake** | Minimum stake the Elector would accept. Below this — no rewards. |
| **frozen_stake** | Stake locked in the previous validation round. |
| **free_pool_balance** | Funds available for staking on the pool balance. |
| **current_stake** | Stake already submitted to the current election (0 if none). |
| **available** | `frozen_stake + free_pool_balance + current_stake` |
| **half** | `available / 2` |
| **sleep_period** | Minimum wait time (fraction of election duration) before acting. |
| **waiting_period** | Maximum wait time for enough participants before using fallback data. Must be >= `sleep_period`. |

---

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

```
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

---

### Configuration

| Parameter | Type | Description |
|---|---|---|
| `sleep_period` | float (0.0–1.0) | Fraction of election duration to wait before acting. |
| `waiting_period` | float (0.0–1.0) | Max fraction of election duration to wait for participants. |

---

### Decision Flowchart

```
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
