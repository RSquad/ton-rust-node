/**
 * TONCore nominator pool: request withdrawal — same V3R2 subwallets as add-nominators-to-pool.ts.
 * pool.fc: op=0, action=119 ('w'). Nominator wallets must be **workchain 0** (same as deposit).
 * Attached TON covers gas / forward fees (see amount_ton).
 *
 * Usage:
 *   bun scripts/withdraw-nominators-from-pool.ts <pool_address> [amount_ton] [count] [action_u8]
 *
 * Env (same as add-nominators-to-pool.ts / topup.ts):
 *   MASTER_WALLET_KEY — 64-byte hex (secret||public)
 *   API_ENDPOINTS — comma-separated HTTP API roots (first entry is used)
 *
 * Optional:
 *   MASTER_WALLET_ID (default 42)
 *   NOMINATOR_SUBWALLET_BASE (default 10000) — must match the deposit script so the same nominators sign
 *   NOMINATOR_WORKCHAIN (default 0 — basechain; required by pool.fc)
 *   NOMINATOR_FUND_EXTRA_TON (default 0.15)
 *   TONCORE_OPCODE_U32 (default 0)
 *   TONCORE_WITHDRAW_ACTION_U8 — if 5th CLI arg omitted (default 119)
 */
import { TonClient } from "@ton/ton";
import { Address, beginCell, fromNano, internal, SendMode, toNano } from "@ton/core";
import { WalletContractV3R2 } from "@ton/ton";
import { checkEnvs } from "./utils";

const DEFAULT_COUNT = 40;
const DEFAULT_SUBWALLET_BASE = 10_000;
const DEFAULT_ACTION_U8 = 119;
const DEFAULT_OPCODE_U32 = 0;
const DEFAULT_NOMINATOR_WORKCHAIN = 0;
/** Default value attached to pool message (gas); withdraw request usually does not move stake in the body. */
const DEFAULT_MSG_VALUE_TON = "0.2";

/** pool.fc: `op == 0`, `action == 119` ('w') */
const TONCORE_ACTION_NOMINATOR_WITHDRAW = 119;

function tonCoreNominatorWithdrawBody() {
    return beginCell().storeUint(0, 32).storeUint(TONCORE_ACTION_NOMINATOR_WITHDRAW, 8).endCell();
}

function tonCorePoolBody(opcodeU32: number, actionU8: number) {
    return beginCell().storeUint(opcodeU32, 32).storeUint(actionU8, 8).endCell();
}

async function run() {
    if (process.argv.length < 3) {
        console.error("Usage: bun scripts/withdraw-nominators-from-pool.ts <pool_address> [amount_ton] [count] [action_u8]");
        console.error("Env: MASTER_WALLET_KEY (64-byte hex), API_ENDPOINTS (comma-separated, same as topup.ts)");
        console.error(
            `Body: uint32 opcode (default ${DEFAULT_OPCODE_U32}) + uint8 action (default ${DEFAULT_ACTION_U8}). ` +
                `Optional env: TONCORE_OPCODE_U32, TONCORE_WITHDRAW_ACTION_U8`,
        );
        console.error(
            `Optional: MASTER_WALLET_ID (default 42), NOMINATOR_SUBWALLET_BASE (default ${DEFAULT_SUBWALLET_BASE}), NOMINATOR_FUND_EXTRA_TON (default 0.15)`,
        );
        process.exit(1);
    }

    checkEnvs(["MASTER_WALLET_KEY", "API_ENDPOINTS"]);

    const poolAddr = Address.parse(process.argv[2]);
    const amountTon = process.argv[3] ?? DEFAULT_MSG_VALUE_TON;
    const count = Number.parseInt(process.argv[4] ?? String(DEFAULT_COUNT), 10);
    const actionU8 = Number.parseInt(
        process.argv[5] ?? process.env.TONCORE_WITHDRAW_ACTION_U8 ?? String(DEFAULT_ACTION_U8),
        10,
    );
    const opcodeU32 = Number.parseInt(process.env.TONCORE_OPCODE_U32 ?? String(DEFAULT_OPCODE_U32), 10);
    const nominatorWorkchain = Number.parseInt(
        process.env.NOMINATOR_WORKCHAIN ?? String(DEFAULT_NOMINATOR_WORKCHAIN),
        10,
    );

    if (!Number.isFinite(count) || count < 1 || count > 500) {
        throw new Error("count must be between 1 and 500");
    }
    if (!Number.isFinite(actionU8) || actionU8 < 0 || actionU8 > 255) {
        throw new Error("action_u8 must be 0..255");
    }
    if (!Number.isFinite(opcodeU32) || opcodeU32 < 0) {
        throw new Error("TONCORE_OPCODE_U32 must be a valid uint32");
    }
    if (nominatorWorkchain !== 0) {
        throw new Error(
            `NOMINATOR_WORKCHAIN must be 0 (basechain). Got ${nominatorWorkchain}`,
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

    const body =
        opcodeU32 === DEFAULT_OPCODE_U32 && actionU8 === DEFAULT_ACTION_U8
            ? tonCoreNominatorWithdrawBody()
            : tonCorePoolBody(opcodeU32, actionU8);

    console.log(`Master wallet: ${masterWallet.address.toString()} (walletId=${masterWalletId})`);
    console.log(
        `Pool: ${poolAddr.toString()}, ${count} withdraw requests × ${amountTon} TON value, ` +
            `body opcode=${opcodeU32} action=${actionU8}, nominator wc=${nominatorWorkchain}, subwallet base=${subwalletBase}`,
    );

    for (let i = 0; i < count; i++) {
        const subId = subwalletBase + i;
        const subW = WalletContractV3R2.create({
            workchain: nominatorWorkchain,
            publicKey,
            walletId: subId,
        });

        console.log(`[${i + 1}/${count}] nominator subwallet id=${subId} → ${subW.address.toString()}`);

        const fundTotal = deployAndFees + amountPer;
        let seqno = await master.getSeqno();
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
        console.log(`  sent withdraw request (${amountTon} TON) body op=${opcodeU32} action=${actionU8}`);
        await new Promise((r) => setTimeout(r, 1500));
    }

    console.log("Done.");
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
