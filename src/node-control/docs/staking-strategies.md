# Nodectl Staking Strategies

## AdaptiveSplit50

### Overview

AdaptiveSplit50 is a staking strategy designed to maximize capital efficiency across validation rounds. The core idea is simple: split all available funds in half and stake each half into alternating election rounds, so that capital is always working. However, this only makes sense when each half is large enough to be selected by the Elector — otherwise the funds sit idle and earn nothing.

**Key principle:** If half of the available funds exceeds the minimum effective stake (the threshold below which the Elector will not select a validator), the strategy stakes half per round. If not, it stakes everything into a single round to avoid leaving idle capital.

---

### Definitions

| Term | Description |
|---|---|
| **Elector** | The TON smart contract that runs validator elections and selects which stakes participate in validation. |
| **Election round** | A time-bounded period during which validators submit stakes. At the end, the Elector selects the validator set. |
| **min_eff_stake** | The minimum effective stake — the lowest stake that the Elector would accept into the validator set. Stakes below this threshold are not selected and earn no rewards. |
| **frozen_stake** | A stake submitted in a previous election that is currently locked (frozen) for the duration of the active validation round. |
| **free_pool_balance** | Uncommitted funds sitting on the nominator pool balance, available for staking. |
| **current_stake** | The stake already submitted to the current election round (0 if nothing has been submitted yet). |
| **available** | Total capital the strategy can work with: `frozen_stake + free_pool_balance + current_stake`. |
| **half** | Half of the available capital: `available / 2`. |
| **config16** | TON blockchain configuration parameter that defines validator set constraints, including `min_validators` — the minimum number of validators required to form a set. |
| **config15** | TON blockchain configuration parameter that defines election timing, including the total election duration. |
| **sleep_period** | A configurable minimum delay (from the start of elections) before the strategy takes any action. Expressed as a fraction of the total election duration. |
| **waiting_period** | A configurable maximum time the strategy will wait for enough participants to appear before falling back to historical data. Expressed as a fraction of the total election duration. `sleep_period <= waiting_period`. |
| **tick** | One iteration of the strategy's main loop. The strategy runs periodically and re-evaluates its position on each tick. |

---

### Algorithm

The algorithm executes in four steps each time a new election round begins.

#### Step 1 — Estimate min_eff_stake from the current election

**Goal:** Determine the minimum effective stake for the current election by emulating the Elector's selection algorithm on the participants who have already submitted their stakes.

1. When a new election starts, the strategy begins monitoring the list of participants who have submitted stakes to the Elector.

2. The strategy waits until **both** of the following conditions are met:
   - At least `config16.min_validators` participants have submitted their stakes (the minimum needed to emulate a meaningful election).
   - The `sleep_period` has elapsed since the start of the election.

3. **Timeout:** If the `waiting_period` elapses and fewer than `min_validators` participants have appeared, the strategy stops waiting and proceeds to Step 2 without a current-election estimate. This prevents the strategy from stalling indefinitely.

4. Once both conditions are satisfied, the strategy emulates an election: it takes the current list of participants, adds its own potential stake (`half = available / 2`) to the list, and runs the Elector's selection algorithm. The result is `curr_min_eff_stake` — the estimated minimum effective stake for this election.

#### Step 2 — Estimate min_eff_stake from the previous election

**Goal:** Obtain a baseline minimum effective stake from historical data, independent of the current election's progress.

1. The strategy calls the Elector's `past_elections` get-method to retrieve the participant map from the most recent completed election.

2. It emulates the Elector's selection algorithm on that historical participant list to compute `prev_min_eff_stake`.

3. This value is **cached** so it does not need to be recomputed on every tick (it does not change within a single election round).

> **Note:** This step always produces a result, unlike Step 1 which may time out. This ensures the strategy always has at least one min_eff_stake estimate to work with.

#### Step 3 — Decide the stake amount and submit

**Goal:** Choose the optimal stake amount and submit it to the Elector.

1. **Pick the conservative estimate.** If both `curr_min_eff_stake` (from Step 1) and `prev_min_eff_stake` (from Step 2) are available, the strategy uses the **smaller** of the two. If only `prev_min_eff_stake` is available (Step 1 timed out), it uses that. This conservative approach reduces the risk of submitting a stake that is too low.

2. **Calculate available funds:**
   ```
   available = frozen_stake + free_pool_balance + current_stake
   ```

3. **Calculate half:**
   ```
   half = available / 2
   ```

4. **Submit the stake:**
   - If `half >= min_eff_stake` → submit `half` to the Elector. The expectation is that the remaining half will be sufficient for the next round. However, this is **not guaranteed** — the stake distribution may change in the next election, shifting the min_eff_stake up or down. Since the future state is unpredictable, the strategy uses the current estimate as the best available approximation.
   - If `half < min_eff_stake` → submit **all available free funds** (`free_pool_balance`) to the Elector. Since `half < min_eff_stake` implies `available < 2 × min_eff_stake`, the remainder after staking any amount would be less than `min_eff_stake` — not enough to participate in the next round. Rather than leaving idle capital that cannot earn rewards, the strategy commits everything to the current round.

5. **Insufficient funds guard:** Before submitting, the strategy checks whether `free_pool_balance >= min_eff_stake`. If the pool does not have enough free funds to cover the required stake, the strategy **skips the election entirely** and logs an error indicating that the election will be missed due to insufficient funds. No stake is submitted in this case.

#### Step 4 — Continuously adjust the stake

**Goal:** After the initial submission, keep monitoring the election and top up the stake if conditions change.

On every subsequent tick during the election:

1. **Re-emulate the election** using the full current participant list to get an updated `min_eff_stake`.

2. **Top-up if outbid:** If `min_eff_stake > current_stake`, the strategy sends an additional stake equal to the difference:
   ```
   topup = min_eff_stake - current_stake
   current_stake += topup
   ```
   This ensures the node remains above the selection threshold even as new, larger stakes arrive.

3. **Go all-in if next round is unviable:** The strategy checks whether the remaining funds (not staked in this round) would be enough to participate in the next election:
   ```
   remaining = (frozen_stake + free_pool_balance) - current_stake
   ```
   If `remaining < min_eff_stake`, it means the leftover funds won't be sufficient to enter the next validator set anyway. In this case, the strategy stakes the entire remaining balance into the current election to maximize returns from this round rather than leaving funds idle.

---

### Configuration Parameters

| Parameter | Type | Description |
|---|---|---|
| `sleep_period` | float (0.0–1.0) | Minimum fraction of the election duration to wait before acting, even if enough participants are present. |
| `waiting_period` | float (0.0–1.0) | Maximum fraction of the election duration to wait for `min_validators` participants. Must be >= `sleep_period`. |

---

### Summary: Decision Flowchart

```
Election starts
       │
       ▼
  Wait for sleep_period AND min_validators participants
       │
       ├─ Both met ──► Emulate election → curr_min_eff_stake
       │
       └─ Timeout ───► curr_min_eff_stake = None
       │
       ▼
  Fetch past_elections → prev_min_eff_stake (cached)
       │
       ▼
  min_eff_stake = min(curr_min_eff_stake, prev_min_eff_stake)
       │
       ▼
  available = frozen_stake + free_pool_balance + current_stake
  half = available / 2
       │
       ├─ half >= min_eff_stake ──► Stake half
       │
       └─ half < min_eff_stake ──► Stake all (next round unviable anyway)
       │
       ▼
  ┌─ On every tick: ──────────────────────────────────┐
  │                                                    │
  │  Re-emulate election → updated min_eff_stake       │
  │                                                    │
  │  If min_eff_stake > current_stake:                 │
  │     top up by (min_eff_stake - current_stake)      │
  │                                                    │
  │  If remaining funds < min_eff_stake:               │
  │     stake all remaining into current round         │
  └────────────────────────────────────────────────────┘
```
