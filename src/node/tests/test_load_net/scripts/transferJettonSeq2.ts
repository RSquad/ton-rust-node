import { Address, toNano } from "@ton/core";
import { parseArgs } from "util";
import { AppGlobals } from "./globals";
import { Wallet } from "./wallet";
import { runWithLimit } from "./batchedPromise";
import { fromJettons, shuffle, toJettons } from "./utils";
import { TimeMeasure } from "./timeMeasure";
import { exit } from "process";

const JETTON_MINTER = Address.parse(process.env.JETTON_MINTER!);
const JETTON_DECIMALS = 9;
const FORWARD_TON = 0.01;
const FORWARD_COMMENT = '';
const TTL_TIMEOUT = 3 * 60 * 1000;//ms

type TransferInfo = {
    lostExternalInMessages: number;
};

class TimeoutError extends Error {
    constructor(message = "Operation timed out") {
        super(message);
        this.name = "TimeoutError";
    }
}

function withTimeout<T>(
    promise: Promise<T>,
    ms: number,
    timeoutMessage?: string
): Promise<T> {
    let timeoutId: ReturnType<typeof setTimeout>;

    const timeoutPromise = new Promise<never>((_, reject) => {
        timeoutId = setTimeout(() => {
            reject(new TimeoutError(timeoutMessage));
        }, ms);
    });

    return Promise.race([promise, timeoutPromise]).finally(() => {
        clearTimeout(timeoutId);
    }) as Promise<T>;
}

/**
 * Example: bun run ./scripts/transferJettonSeq2.ts --count 100 --jettons 0.01
 */

export async function run() {
    // Read arguments
    const { values, positionals } = parseArgs({
        options: {
            count: { type: 'string', short: 'c' },      // transfers count
            jettons: { type: 'string', short: 'j' },
        },
        allowPositionals: true,
    });

    const transfersCount = Number.parseInt(values.count!);
    const jettonPerTransfer = Number.parseFloat(values.jettons ?? "0.01");

    // Load wallets
    console.log(`Load wallets...`);
    let walletStorage = (await AppGlobals.S()).getWalletStorage();
    let wallets = await walletStorage.readAll();
    console.log(`Total wallets: ${wallets.length}`);

    // Start test
    const start = performance.now();
    await TestJettonTransfer(
        wallets,
        transfersCount,
        jettonPerTransfer
    );
    const end = performance.now();

    console.log(`All tests DONE! took ${(end - start).toFixed(2)} ms (${((end - start) / (1000.0 * 60)).toFixed(2)} min)`);
}

async function TestJettonTransfer(
    wallets: Wallet[],
    transfersCount: number,
    jettonPerTransfer: number,
): Promise<void> {
    if (wallets.length == 0) {
        console.error(`Wallets count is 0`);
    }

    const apiBatchSize = (await AppGlobals.S()).getApiBatchSize();
    console.log(`apiBatchSize = ${apiBatchSize}`);
    let timeMeasure = new TimeMeasure({ keepSamples: true, maxSamplesPerName: 10000 });

    // Update jetton wallet address
    {
        let promises: Promise<void>[] = [];
        for (let wallet of wallets) {
            let promise = wallet.setJettonMinter("Test", JETTON_MINTER);
            promises.push(promise);
        }

        console.log(`Update jetton wallet address: runWithLimit (begin)...`);
        await runWithLimit(apiBatchSize, () => { return promises.pop(); })
        console.log(`runWithLimit (end)...`);
    }

    // Transfer jettons
    let lostExternalInMessages = 0;
    let i = 0;
    while (i < transfersCount) {
        // Shuffle wallets
        wallets = shuffle(wallets);

        const splitByIndexParity = (wallets: readonly Wallet[]): [even: Wallet[], odd: Wallet[]] => {
            return wallets.reduce<[Wallet[], Wallet[]]>((acc, w, i) => {
                (i % 2 ? acc[1] : acc[0]).push(w);
                return acc;
            }, [[], []]);
        };

        let [walletsA, walletsB] = splitByIndexParity(wallets);
        let walletIdxA = 0;
        let walletIdxB = 0;

        while (i < transfersCount) {
            let walletA = walletsA[walletIdxA];
            let walletB = walletsB[walletIdxB];
            let promise = Transfer(walletA, walletB, jettonPerTransfer, timeMeasure, TTL_TIMEOUT);

            let info = await promise;
            lostExternalInMessages += info.lostExternalInMessages;

            i++;
            walletIdxA++;
            walletIdxB++;

            if ((walletIdxA >= walletsA.length) || (walletIdxB >= walletsB.length)) {
                break;
            }
        }
    }

    console.log(`\n\n\nlostExternalInMessages = ${lostExternalInMessages}`);
    console.table(timeMeasure.snapshot());
}

async function Transfer(
    walletFrom: Wallet,
    walletTo: Wallet,
    jettonPerTransfer: number,
    timeMeasure: TimeMeasure,
    ttlTimeout: number, // ms
): Promise<TransferInfo> {
    let lostExternalInMessages = 0;

    // Receiver jetton balance
    {
        console.log(`[Transfer]: update receiver '${walletTo.getJettonWalletAddr("Test")}' jetton balance = ${fromJettons(walletTo.getJettonBalance("Test"), JETTON_DECIMALS)} J (begin)...`);
        let tmId = timeMeasure.start("updateJettonBalance");
        await walletTo.updateJettonBalance("Test");
        timeMeasure.stop(tmId);
        console.log(`[Transfer]: update receiver '${walletTo.getJettonWalletAddr("Test")}' jetton balance = ${fromJettons(walletTo.getJettonBalance("Test"), JETTON_DECIMALS)} J (end)...`);
    }

    // Sender seqno
    {
        console.log(`[Transfer]: update sender '${walletFrom.getAddress()}' seqno = ${walletFrom.getSeqno()} (begin)...`);
        let tmId = timeMeasure.start("updateSeqno");
        await walletFrom.updateSeqno();
        timeMeasure.stop(tmId);
        console.log(`[Transfer]: update sender '${walletFrom.getAddress()}' seqno = ${walletFrom.getSeqno()} (end)...`);
    }

    // Send Jettons
    while (true) {
        let tmUserExpId = timeMeasure.start("userExpTransferJettons");
        {
            console.log(`[Transfer]: sendJetton ${jettonPerTransfer} from '${walletFrom.getAddress()}' to '${walletTo.getAddress()}' (begin)...`);
            let tmId = timeMeasure.start("sendJetton");
            let extInMsg = await walletFrom.sendJetton(
                "Test",
                walletTo.getAddress(),
                toJettons(jettonPerTransfer, JETTON_DECIMALS),
                toNano(FORWARD_TON),
                toNano('0.2'),  // Fee
                FORWARD_COMMENT
            );
            timeMeasure.stop(tmId);
            console.log(`message id: ${Wallet.messageIdToHex(extInMsg)}`);
            console.log(`message base64: ${Wallet.extInMessageToBase64(extInMsg)}`);

            console.log(`[Transfer]: sendJetton ${jettonPerTransfer} from '${walletFrom.getAddress()}' to '${walletTo.getAddress()}' (end)...`);
        }

        // Wait for sender seqno update
        {
            let seqno = walletFrom.getSeqno();
            console.log(`[Transfer]: wait for sender seqno update '${walletFrom.getAddress()}' seqno = ${seqno} (begin)...`);
            let tmId = timeMeasure.start("waitForSeqNoChange");

            try {
                await withTimeout(
                    walletFrom.waitForSeqNoChange(seqno),
                    ttlTimeout,
                    "waitForSeqNoChange took too long, restart"
                );
            } catch (err) {
                if (err instanceof TimeoutError) {
                    console.warn("Timed out:", err.message);
                    lostExternalInMessages++;
                    timeMeasure.stopErr(tmId);
                    timeMeasure.stopErr(tmUserExpId);
                    continue;
                } else {
                    exit(1);
                }
            }

            timeMeasure.stop(tmId);
            console.log(`[Transfer]: wait for sender seqno update '${walletFrom.getAddress()}' seqno = ${walletFrom.getSeqno()} (end)...`);
        }

        // Wait for receiver jetton balance update
        {
            console.log(`[Transfer]: wait for receiver '${walletTo.getAddress()}' jetton balance update = ${fromJettons(walletTo.getJettonBalance("Test"), JETTON_DECIMALS)} J (begin)...`);
            let tmId = timeMeasure.start("waitForJettonBalanceChange");

            try {
                await withTimeout(
                    walletTo.waitForJettonBalanceChange("Test"),
                    ttlTimeout,
                    "waitForSeqNoChange took too long, restart"
                );
            } catch (err) {
                if (err instanceof TimeoutError) {
                    console.warn("Timed out:", err.message);
                    lostExternalInMessages++;
                    timeMeasure.stopErr(tmId);
                    timeMeasure.stopErr(tmUserExpId);
                    continue;
                } else {
                    exit(1);
                }
            }

            timeMeasure.stop(tmId);
            timeMeasure.stop(tmUserExpId);
            console.log(`[Transfer]: wait for receiver '${walletTo.getAddress()}' jetton balance update = ${fromJettons(walletTo.getJettonBalance("Test"), JETTON_DECIMALS)} J (end)...`);
        }

        break;
    }

    return {
        lostExternalInMessages: lostExternalInMessages,
    };
}

run();
