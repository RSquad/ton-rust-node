# First elections with Rust node

`nodectl` only works with Rust TON Node. If you are currently running a C++ TON Node, you will need to migrate your cluster to Rust TON Node.

## Migration scenario

Prerequisites:

- C++ TON Node is running and validating in the current round, meaning it has a frozen stake.
- Rust TON Node is running and fully synced with the network.

Goal:

- Make the Rust TON Node a validator in the next validation period.
- Shut down the C++ node after the current validation period ends.

Steps:

1. Deploy the Rust node and let it sync with the network (if not already done).
2. Deploy nodectl and configure it to work with the Rust node:

    - Add the node's control server
    - Import the validator wallet key into the vault and add the wallet to the nodectl config (the same wallet used by the C++ node)
    - Add the nominator pool to the nodectl config (the same pool used by the C++ node)
    - Configure the binding

3. Disable election participation on the C++ node.
4. Wait for the next election round to begin.

Nodectl will automatically withdraw the unfrozen stake previously submitted by the C++ node in the prior elections and submit a new stake to the current elections according to the staking policy.

---

### Critical — read before the first Rust elections

> [!CAUTION]
> If the node's staking policy in `nodectl` is set to `split50` (the default), `nodectl` first calculates the total available funds for staking = frozen stake + pool's free balance + already submitted stake, then splits them in half and submits that amount to the current elections. However, since the Rust node does not have the validator key that the C++ node is currently using to validate in this round, `nodectl` cannot determine the frozen stake and will treat it as 0. **As a result, the stake amount will be half of what it should be.**

For example:

```bash
C++ node frozen stake = 1_000_000 TON
Pool free balance = 1_000_000 TON
Elections started and nodectl has not submitted any stakes yet: already submitted stake = 0 TON
# Since Rust node does not have the current validator key:
Rust node frozen stake = 0 TON
nodectl calculates total available funds = 0 TON + 1_000_000 TON + 0 TON = 1_000_000 TON
Splits in half and submits to current elections = 1_000_000 / 2 = 500_000 TON
```

This situation only occurs during the first election in which the Rust node participates. After the C++ stake is unfrozen (in the following elections), `nodectl` will return the unfrozen stake to the pool balance and will be able to calculate the correct stake amount.

---

**How to top up the remaining funds into the stake?**

Use the manual staking command:

```bash
nodectl config wallet stake -b <name> -a <tons> [-m <max-factor>]
```

This command allows you to manually submit a stake to the current elections or top up the remaining funds into an existing stake.