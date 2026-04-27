/**
 * TONCore nominator pool: send N internal messages from distinct V3R2 subwallets.
 *
 * Pool FunC (`pool.fc`): nominator deposit uses **`op == 0`**, **`action == 100`** ('d'), value in message.
 * **`throw_unless(61, sender_wc == 0)`** — nominators must use **basechain (workchain 0)** wallets, not masterchain.
 *
 * CI / throughput: deploy+fund uses a **Highload Wallet v3** on masterchain (same key material as `MASTER_WALLET_KEY`)
 * in **one** `sendBatch` external carrying all deploy+fund `internal` messages; stake messages are then sent **in parallel**
 * from each nominator contract (each signs as its own V3R2). Large `count` / HTTP limits may return 500 — then raise gas / reduce N or use master-only path manually.
 *
 * Usage:
 *   bun scripts/add-nominators-to-pool.ts <pool_address> [amount_ton] [count]
 *
 * Env (same as scripts/topup.ts):
 *   MASTER_WALLET_KEY — 64-byte hex (secret||public)
 *
 * Stake:
 *   Pool charges 1 TON `DEPOSIT_PROCESSING_FEE` before crediting. Default **10001** TON attached so **10000**
 *   remains when min_nominator_stake is 10000. Override argv or NOMINATOR_STAKE_TON.
 *
 * Optional:
 *   MASTER_WALLET_ID (default 42)
 *   NOMINATOR_SUBWALLET_BASE (default 10000)
 *   NOMINATOR_WORKCHAIN (default **0** — required by pool; do not use -1 for nominators)
 *   NOMINATOR_FUND_EXTRA_TON (default 0.15)
 *   POOL_INFO_DELAY_MS — wait before get_pool_data dump (default 5000)
 *   TONCORE_POOL_INFO_EXTRA — comma-separated extra pool addresses to print after (same API)
 *
 * Highload: each run starts `HighloadWalletV3.newSequence()`; reusing the same on-chain highload contract must match the next query id (see logged line after `sendBatch`).
 */
import { TonClient } from "@ton/ton";
import { Address, beginCell, fromNano, internal, SendMode, toNano } from "@ton/core";
import { WalletContractV3R2 } from "@ton/ton";
import { HighloadWalletV3 } from "@tonkite/highload-wallet-v3";
import { checkEnvs } from "./utils";
import {
    fetchMinNominatorStakeNano,
    fetchTonCorePoolSnapshot,
    formatTonCorePoolSnapshot,
} from "./toncore_pool_info";

const DEFAULT_COUNT = 40;
const DEFAULT_SUBWALLET_BASE = 10_000;
/** Basechain — required by pool.fc for nominator deposit/withdraw. */
const DEFAULT_NOMINATOR_WORKCHAIN = 0;
/** Attached TON per msg; must be >= on-chain min_nominator_stake + 1 TON pool fee (default min stake 10k → 10001). */
const DEFAULT_STAKE_TON = "10001";
const DEFAULT_POOL_INFO_DELAY_MS = 5000;

/** Highload wallet timeout (seconds). */
const HIGHLOAD_TIMEOUT_SEC = 60 * 60 * 24;
/** After master→highload deploy/topup: poll until active and funded. */
const HIGHLOAD_DEPLOY_FUND_WAIT_MS = 600_000;
/** After highload `sendBatch`: wait until nominator wallets answer `seqno`. */
const NOMINATOR_HL_DEPLOY_WAIT_MS = 600_000;
/** Final poll until every nominator wallet is active. */
const NOMINATOR_DEPLOY_WAIT_MS = 600_000;

/** pool.fc: `op == 0`, `action == 100` ('d') */
const TONCORE_ACTION_NOMINATOR_DEPOSIT = 100;

function tonCoreNominatorDepositBody() {
    return beginCell().storeUint(0, 32).storeUint(TONCORE_ACTION_NOMINATOR_DEPOSIT, 8).endCell();
}

function uniqueAddresses(primary: Address, extras: Address[]): Address[] {
    const out: Address[] = [primary];
    for (const a of extras) {
        if (!out.some((x) => x.equals(a))) {
            out.push(a);
        }
    }
    return out;
}


/** Some HTTP stacks throw or return non-zero on get-method for missing/uninit accounts. */
async function contractGetMethodOk(client: TonClient, address: Address, name: string): Promise<boolean> {
    try {
        const res = await client.runMethodWithError(address, name, []);
        return res.exit_code === 0;
    } catch {
        return false;
    }
}

/** One pass: who already answers `seqno` get-method (deployed/active) vs who does not yet. */
async function classifyWalletsBySeqnoReadiness(
    client: TonClient,
    wallets: WalletContractV3R2[],
): Promise<{ ready: number; withoutSeqno: WalletContractV3R2[] }> {
    const withoutSeqno: WalletContractV3R2[] = [];
    for (const w of wallets) {
        if (await contractGetMethodOk(client, w.address, "seqno")) continue;
        withoutSeqno.push(w);
    }
    return { ready: wallets.length - withoutSeqno.length, withoutSeqno };
}

/** Poll until each wallet answers get-method `seqno` (exit 0), or timeout. */
async function waitUntilAllWalletsSeqnoReady(
    client: TonClient,
    wallets: WalletContractV3R2[],
    label: string,
    maxMs: number,
    options?: {
        pollMs?: number;
        /** Called when not all ready yet, before sleeping until next poll. */
        onProgress?: (ready: number, total: number) => void | Promise<void>;
        logSuccess?: boolean;
    },
): Promise<void> {
    const pollMs = options?.pollMs ?? 2500;
    const t0 = Date.now();
    while (Date.now() - t0 < maxMs) {
        const { ready } = await classifyWalletsBySeqnoReadiness(client, wallets);
        if (ready === wallets.length) {
            if (options?.logSuccess !== false) {
                console.log(`    "${label}": all ${wallets.length} wallet(s) active`);
            }
            return;
        }
        await options?.onProgress?.(ready, wallets.length);
        await new Promise((r) => setTimeout(r, pollMs));
    }
    const { ready } = await classifyWalletsBySeqnoReadiness(client, wallets);
    throw new Error(`"${label}": only ${ready}/${wallets.length} wallet(s) active within ${maxMs} ms`);
}

/** Calls `predicate` on an interval until it returns true or `timeoutMs`; if `predicate` throws, that tick counts as false and polling continues. */
async function pollUntil(
    label: string,
    predicate: () => Promise<boolean>,
    timeoutMs: number,
    pollMs: number,
): Promise<void> {
    const start = Date.now();
    while (Date.now() - start < timeoutMs) {
        if (await predicate().catch(() => false)) {
            return;
        }
        await new Promise((r) => setTimeout(r, pollMs));
    }
    throw new Error(`Timeout waiting for: ${label} (${timeoutMs} ms)`);
}

async function run() {
    if (process.argv.length < 3) {
        console.error("Usage: bun scripts/add-nominators-to-pool.ts <pool_address> [amount_ton] [count]");
        console.error("Env: MASTER_WALLET_KEY (64-byte hex), API_ENDPOINTS (comma-separated, same as topup.ts)");
        console.error(
            `Body: op=0 + action=${TONCORE_ACTION_NOMINATOR_DEPOSIT} ('d'). Nominator wallets must be workchain ${DEFAULT_NOMINATOR_WORKCHAIN}.`,
        );
        console.error(
            `Default stake: ${DEFAULT_STAKE_TON} TON (override argv or NOMINATOR_STAKE_TON). ` +
                `Optional: MASTER_WALLET_ID (default 42), NOMINATOR_SUBWALLET_BASE (default ${DEFAULT_SUBWALLET_BASE}), ` +
                `NOMINATOR_WORKCHAIN (default ${DEFAULT_NOMINATOR_WORKCHAIN}), NOMINATOR_FUND_EXTRA_TON (default 0.15), ` +
                `POOL_INFO_DELAY_MS, TONCORE_POOL_INFO_EXTRA`,
        );
        process.exit(1);
    }

    checkEnvs(["MASTER_WALLET_KEY", "API_ENDPOINTS"]);

    const poolAddr = Address.parse(process.argv[2]);
    const amountTon = process.argv[3] ?? process.env.NOMINATOR_STAKE_TON ?? DEFAULT_STAKE_TON;
    const count = Number.parseInt(process.argv[4] ?? String(DEFAULT_COUNT), 10);
    const poolInfoDelayMs = Number.parseInt(process.env.POOL_INFO_DELAY_MS ?? String(DEFAULT_POOL_INFO_DELAY_MS), 10);
    const nominatorWorkchain = Number.parseInt(
        process.env.NOMINATOR_WORKCHAIN ?? String(DEFAULT_NOMINATOR_WORKCHAIN),
        10,
    );

    if (!Number.isFinite(count) || count < 1 || count > 40) {
        throw new Error("count must be between 1 and 40");
    }
    if (!Number.isFinite(poolInfoDelayMs) || poolInfoDelayMs < 0) {
        throw new Error("POOL_INFO_DELAY_MS must be >= 0");
    }
    if (nominatorWorkchain !== 0) {
        throw new Error(
            `NOMINATOR_WORKCHAIN must be 0 (basechain). Pool rejects nominator ops from masterchain (see pool.fc throw 61). Got ${nominatorWorkchain}`,
        );
    }

    const amountPer = toNano(amountTon);
    const masterKey = Buffer.from(process.env.MASTER_WALLET_KEY!, "hex");
    if (masterKey.length !== 64) {
        throw new Error("MASTER_WALLET_KEY must be 64 bytes (hex)");
    }
    const publicKey = masterKey.subarray(32);
    const masterWalletId = Number.parseInt(process.env.MASTER_WALLET_ID ?? "42", 10);
    const subwalletBase = Number.parseInt(process.env.NOMINATOR_SUBWALLET_BASE ?? String(DEFAULT_SUBWALLET_BASE), 10);
    const deployAndFees = toNano(process.env.NOMINATOR_FUND_EXTRA_TON ?? "0.15");

    const masterWallet = WalletContractV3R2.create({
        workchain: -1,
        publicKey,
        walletId: masterWalletId,
    });

    const client = new TonClient({
        endpoint: process.env.API_ENDPOINTS!.split(",")[0] + "jsonRPC",
    });
    const master = client.open(masterWallet);

    const minNom = await fetchMinNominatorStakeNano(client, poolAddr);
    /** pool.fc: `msg_value -= DEPOSIT_PROCESSING_FEE()` (1 TON) before crediting nominators dict */
    const depositProcessingFee = toNano("1");
    const credited = amountPer - depositProcessingFee;
    if (credited <= 0n) {
        throw new Error(`Attached value must exceed 1 TON deposit processing fee (send more than 1 TON).`);
    }
    if (credited < minNom) {
        throw new Error(
            `After 1 TON pool deposit fee, ${fromNano(credited)} TON remains; need >= min_nominator_stake ${fromNano(minNom)} TON. Increase amount_ton by at least 1 TON.`,
        );
    }

    const extraPoolAddrs = (process.env.TONCORE_POOL_INFO_EXTRA ?? "")
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean)
        .map((a) => Address.parse(a));
    const poolsToReport = uniqueAddresses(poolAddr, extraPoolAddrs);

    const body = tonCoreNominatorDepositBody();

    const highloadTopup = (deployAndFees + amountPer + toNano("1")) * BigInt(count);

    const highloadWallet = new HighloadWalletV3(HighloadWalletV3.newSequence(), publicKey, HIGHLOAD_TIMEOUT_SEC, HighloadWalletV3.DEFAULT_SUBWALLET_ID, -1);

    console.log(`Master wallet: ${masterWallet.address.toString()} (walletId=${masterWalletId})`);
    console.log(`Highload wallet (orchestrator, mc): ${highloadWallet.address.toString()} (subwalletId=${HighloadWalletV3.DEFAULT_SUBWALLET_ID}, DEFAULT_SUBWALLET_ID)`);
    console.log(
        `Pool: ${poolAddr.toString()}, ${count} nominators × ${amountTon} TON, ` +
            `body op=0 action=${TONCORE_ACTION_NOMINATOR_DEPOSIT} ('d'), nominator wc=${nominatorWorkchain}, subwallet base=${subwalletBase}`,
    );
    console.log(`On-chain min_nominator_stake: ${fromNano(minNom)} TON`);

    const nominatorWallets: WalletContractV3R2[] = [];
    const batchMessages: { mode: SendMode; message: ReturnType<typeof internal> }[] = [];
    for (let i = 0; i < count; i++) {
        const subId = subwalletBase + i;
        const subW = WalletContractV3R2.create({
            workchain: nominatorWorkchain,
            publicKey,
            walletId: subId,
        });
        nominatorWallets.push(subW);
        console.log(`[${i + 1}/${count}] nominator subwallet id=${subId} → ${subW.address.toString()}`);
        batchMessages.push({
            mode: SendMode.PAY_GAS_SEPARATELY,
            message: internal({
                to: subW.address,
                value: deployAndFees + amountPer,
                bounce: false,
                init: subW.init,
            }),
        });
    }

    const totalOutFromHighload = (deployAndFees + amountPer) * BigInt(count);
    const valuePerBatch = totalOutFromHighload + toNano("1");
    /** Enough for `sendBatch` attach + headroom for MC deploy/import fees (not full nominal `highloadTopup`). */
    const highloadFundedMinBalance = valuePerBatch + toNano("5");

    const hlDeployed = await client.isContractDeployed(highloadWallet.address).catch(() => false);
    const hlBalance = await client.getBalance(highloadWallet.address);

    if (!hlDeployed || hlBalance < highloadFundedMinBalance) {
        const seqno = await master.getSeqno();
        try {
            await master.sendTransfer({
                seqno,
                secretKey: masterKey,
                messages: [
                    internal({
                        to: highloadWallet.address,
                        value: highloadTopup,
                        bounce: false,
                        init: hlDeployed ? undefined : highloadWallet.init,
                    }),
                ],
                sendMode: SendMode.PAY_GAS_SEPARATELY,
            });
        } catch (e) {
            throw new Error(e instanceof Error ? e.message : String(e));
        }
        console.log(
            hlDeployed
                ? `  Topped up highload with ${fromNano(highloadTopup)} TON (deploy already active)`
                : `  Deploy+fund highload with ${fromNano(highloadTopup)} TON, waiting…`,
        );
        await pollUntil(
            "highload deployed + funded",
            async () => {
                if (!(await client.isContractDeployed(highloadWallet.address))) return false;
                const b = await client.getBalance(highloadWallet.address);
                return b >= highloadFundedMinBalance;
            },
            HIGHLOAD_DEPLOY_FUND_WAIT_MS,
            2000,
        );
    }

    const highloadContract = client.open(highloadWallet);
    const createdAt = Math.floor(Date.now() / 1000) - 10;
    console.log(`  Highload deploy+fund: one sendBatch with ${count} messages (single external)…`);
    try {
        await highloadContract.sendBatch(masterKey, {
            messages: batchMessages,
            valuePerBatch,
            createdAt,
        });
    } catch (e) {
        throw new Error(e instanceof Error ? e.message : String(e));
    }
    highloadWallet.sequence.next();
    try {
        await waitUntilAllWalletsSeqnoReady(client, nominatorWallets, "hl-deploy", NOMINATOR_HL_DEPLOY_WAIT_MS);
    } catch (err) {
        const hlB = await client.getBalance(highloadWallet.address).catch(() => 0n);
        console.log(
            `  ${err instanceof Error ? err.message : String(err)} · highload ${fromNano(hlB)} TON — not all nominators answered seqno after sendBatch within timeout; continuing (check RPC / balances / count).`,
        );
    }

    console.log(
        `  Next highload query id (local sequence after sendBatch; reuse same on-chain highload only with matching query id): ${highloadWallet.sequence.current()}`,
    );
    await new Promise((r) => setTimeout(r, 3000));

    console.log(
        `  Waiting for all nominator wallets to become active (poll, max ${NOMINATOR_DEPLOY_WAIT_MS / 1000}s)…`,
    );
    try {
        await waitUntilAllWalletsSeqnoReady(client, nominatorWallets, "deploy-wait", NOMINATOR_DEPLOY_WAIT_MS, {
            pollMs: 3000,
            logSuccess: false,
            onProgress: async (ready, total) => {
                try {
                    const hlB = await client.getBalance(highloadWallet.address);
                    console.log(
                        `  Nominator contracts ready (get seqno): ${ready}/${total} · highload balance ${fromNano(hlB)} TON`,
                    );
                } catch {
                    console.log(`  Nominator contracts ready (get seqno): ${ready}/${total} · highload balance: RPC error`);
                }
            },
        });
    } catch {
        const { ready, withoutSeqno } = await classifyWalletsBySeqnoReadiness(client, nominatorWallets);
        const stuck = withoutSeqno.map((w) => {
            const i = nominatorWallets.indexOf(w);
            return `#${i + 1} ${w.address.toString()}`;
        });
        const hlBal = await client.getBalance(highloadWallet.address);
        const lowHl = hlBal < toNano("50");
        const hint = lowHl
            ? `Highload balance ${fromNano(hlBal)} TON is low — top up the master and re-run (intended highload topup this run: ${fromNano(highloadTopup)} TON).`
            : `Highload still holds ${fromNano(hlBal)} TON — try lowering count if ton-http-api rejects large BOC (HTTP 500), or wait for the chain.`;
        throw new Error(
            `Only ${ready}/${count} nominator wallets became active after ${NOMINATOR_DEPLOY_WAIT_MS} ms. ${hint} Stuck: ${stuck.join("; ")}`,
        );
    }
    console.log(`  All ${count} nominator wallets active.`);

    console.log(`  Sending ${count} stake transfers in parallel (each from its own nominator wallet)…`);
    await Promise.all(
        nominatorWallets.map(async (subW, idx) => {
            const sub = client.open(subW);
            const subSeqno = await sub.getSeqno();
            await sub.sendTransfer({
                seqno: subSeqno,
                secretKey: masterKey,
                messages: [
                    internal({
                        to: poolAddr,
                        value: amountPer,
                        bounce: true,
                        body,
                    }),
                ],
                sendMode: SendMode.PAY_GAS_SEPARATELY,
            });
            console.log(`  [${idx + 1}/${count}] sent ${amountTon} TON (op=0 action=${TONCORE_ACTION_NOMINATOR_DEPOSIT})`);
        }),
    );

    console.log("Done.");
    if (poolInfoDelayMs > 0) {
        console.log(`Waiting ${poolInfoDelayMs} ms before reading pool state…`);
        await new Promise((r) => setTimeout(r, poolInfoDelayMs));
    }
    console.log("--- Pool snapshot(s) (get_pool_data) ---");
    for (const a of poolsToReport) {
        try {
            const snap = await fetchTonCorePoolSnapshot(client, a);
            console.log(formatTonCorePoolSnapshot(snap));
            console.log("");
        } catch (e) {
            console.error(`get_pool_data failed for ${a.toString()}: ${e}`);
        }
    }
}

if (import.meta.main) {
    (async () => {
        try {
            await run();
        } catch (error) {
            console.error(`Error: ${error}`);
            process.exit(1);
        }
    })();
}
