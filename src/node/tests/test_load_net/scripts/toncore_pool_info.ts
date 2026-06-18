/**
 * Read TONCore nominator pool on-chain snapshot via get_pool_data (same layout as
 * node-control/contracts nominator wrappers).
 */
import type { TonClient } from "@ton/ton";
import { Address, TupleReader, fromNano } from "@ton/core";

export interface TonCorePoolSnapshot {
    address: string;
    balance: bigint;
    state: number;
    nominatorsCount: number;
    stakeAmountSent: bigint;
    validatorAmount: bigint;
    validatorAddress: string;
    validatorRewardShare: number;
    maxNominatorsCount: number;
    minValidatorStake: bigint;
    minNominatorStake: bigint;
    stakeAt: number;
    savedValidatorSetHashHex: string;
    validatorSetChangesCount: number;
    validatorSetChangeTime: bigint;
    stakeHeldFor: bigint;
}

function hex64FromBigInt(n: bigint): string {
    const h = n.toString(16);
    return h.length <= 64 ? h.padStart(64, "0") : h.slice(-64);
}

function readValidatorAddress(stack: TupleReader): Address {
    const t = stack.peek().type;
    if (t === "int") {
        const n = stack.readBigNumber();
        return Address.parseRaw(`-1:${hex64FromBigInt(n)}`);
    }
    return stack.readAddress();
}

function hash32HexFromInt(n: bigint): string {
    return hex64FromBigInt(n);
}

/** Parse stack returned by get_pool_data (flat 16 cells or a single tuple of 16). */
export function parseGetPoolDataStack(stack: TupleReader): Omit<TonCorePoolSnapshot, "address" | "balance"> {
    let r = stack;
    if (stack.remaining === 1 && stack.peek().type === "tuple") {
        r = stack.readTuple();
    }

    const state = r.readNumber();
    const nominatorsCount = r.readNumber();
    const stakeAmountSent = r.readBigNumber();
    const validatorAmount = r.readBigNumber();
    const validatorAddress = readValidatorAddress(r).toString();
    const validatorRewardShare = r.readNumber();
    const maxNominatorsCount = r.readNumber();
    const minValidatorStake = r.readBigNumber();
    const minNominatorStake = r.readBigNumber();
    // nominators / withdraw_requests dict cells — HTTP API may return null when empty
    r.readCellOpt();
    r.readCellOpt();
    const stakeAt = r.readNumber();
    const savedHash = r.readBigNumber();
    const validatorSetChangesCount = r.readNumber();
    const validatorSetChangeTime = r.readBigNumber();
    const stakeHeldFor = r.readBigNumber();

    return {
        state,
        nominatorsCount,
        stakeAmountSent,
        validatorAmount,
        validatorAddress,
        validatorRewardShare,
        maxNominatorsCount,
        minValidatorStake,
        minNominatorStake,
        stakeAt,
        savedValidatorSetHashHex: hash32HexFromInt(savedHash),
        validatorSetChangesCount,
        validatorSetChangeTime,
        stakeHeldFor,
    };
}

export async function fetchTonCorePoolSnapshot(client: TonClient, poolAddr: Address): Promise<TonCorePoolSnapshot> {
    const balance = await client.getBalance(poolAddr);
    const { stack } = await client.runMethod(poolAddr, "get_pool_data", []);
    const parsed = parseGetPoolDataStack(stack);
    return { address: poolAddr.toString(), balance, ...parsed };
}

export function formatTonCorePoolSnapshot(s: TonCorePoolSnapshot): string {
    const lines = [
        `  address: ${s.address}`,
        `  balance: ${fromNano(s.balance)} TON`,
        `  state: ${s.state}`,
        `  nominators_count: ${s.nominatorsCount}`,
        `  stake_amount_sent: ${fromNano(s.stakeAmountSent)} TON`,
        `  validator_amount: ${fromNano(s.validatorAmount)} TON`,
        `  validator_address: ${s.validatorAddress}`,
        `  validator_reward_share (bps): ${s.validatorRewardShare}`,
        `  max_nominators: ${s.maxNominatorsCount}`,
        `  min_validator_stake: ${fromNano(s.minValidatorStake)} TON`,
        `  min_nominator_stake: ${fromNano(s.minNominatorStake)} TON`,
        `  stake_at: ${s.stakeAt}`,
        `  saved_validator_set_hash: ${s.savedValidatorSetHashHex}`,
        `  validator_set_changes_count: ${s.validatorSetChangesCount}`,
        `  validator_set_change_time: ${s.validatorSetChangeTime}`,
        `  stake_held_for: ${s.stakeHeldFor}`,
    ];
    return lines.join("\n");
}

export async function fetchMinNominatorStakeNano(client: TonClient, poolAddr: Address): Promise<bigint> {
    const { stack } = await client.runMethod(poolAddr, "get_pool_data", []);
    let r = stack;
    if (stack.remaining === 1 && stack.peek().type === "tuple") {
        r = stack.readTuple();
    }
    r.skip(8);
    return r.readBigNumber();
}
