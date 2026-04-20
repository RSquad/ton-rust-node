/**
 * TONCore nominator pool: send N internal messages from distinct V3R2 subwallets.
 *
 * Pool FunC (`pool.fc`): nominator deposit uses **`op == 0`**, **`action == 100`** ('d'), value in message.
 * **`throw_unless(61, sender_wc == 0)`** — nominators must use **basechain (workchain 0)** wallets, not masterchain.
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
 */
import { TonClient } from "@ton/ton";
import { Address, beginCell, fromNano, internal, SendMode, toNano } from "@ton/core";
import { WalletContractV3R2 } from "@ton/ton";
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

    console.log(`Master wallet: ${masterWallet.address.toString()} (walletId=${masterWalletId})`);
    console.log(
        `Pool: ${poolAddr.toString()}, ${count} msgs × ${amountTon} TON, ` +
            `body op=0 action=${TONCORE_ACTION_NOMINATOR_DEPOSIT} ('d'), nominator wc=${nominatorWorkchain}, subwallet base=${subwalletBase}`,
    );
    console.log(`On-chain min_nominator_stake: ${fromNano(minNom)} TON`);

    for (let i = 0; i < count; i++) {
        const subId = subwalletBase + i;
        const subW = WalletContractV3R2.create({
            workchain: nominatorWorkchain,
            publicKey,
            walletId: subId,
        });

        console.log(`[${i + 1}/${count}] nominator subwallet id=${subId} → ${subW.address.toString()}`);

        const fundTotal = deployAndFees + amountPer;
        const seqno = await master.getSeqno();
        await master.sendTransfer({
            seqno,
            secretKey: masterKey,
            messages: [
                internal({
                    to: subW.address,
                    value: fundTotal,
                    bounce: false,
                    init: subW.init,
                }),
            ],
            sendMode: SendMode.PAY_GAS_SEPARATELY,
        });
        console.log(`  deploy+fund ${fromNano(fundTotal)} TON, waiting…`);
        await new Promise((r) => setTimeout(r, 3000));

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
        console.log(`  sent ${amountTon} TON (op=0 action=${TONCORE_ACTION_NOMINATOR_DEPOSIT})`);
        await new Promise((r) => setTimeout(r, 1500));
    }

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
