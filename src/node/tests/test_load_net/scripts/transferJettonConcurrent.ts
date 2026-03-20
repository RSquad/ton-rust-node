import { Address, toNano } from "@ton/core";
import { parseArgs } from "util";
import { AppGlobals } from "./globals";
import { Wallet } from "./wallet";
import { runWithLimit } from "./batchedPromise";
import { fromJettons, shuffle, toJettons } from "./utils";
import { TimeMeasure } from "./timeMeasure";

const JETTON_MINTER = Address.parse(process.env.JETTON_MINTER!);
const JETTON_DECIMALS = 9;
const FORWARD_TON = 0.01;
const FORWARD_COMMENT = '';

/**
 * Example: bun run ./scripts/transferJettonConcurrent.ts --count 5 --jettons 0.01
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
    await TestJettonTransfer(
        wallets,
        transfersCount,
        jettonPerTransfer
    );

    console.log(`All tests DONE!`);
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
        let transferPromises: Promise<void>[] = [];
        let walletIdxA = 0;
        let walletIdxB = 0;

        while (i < transfersCount) {
            let walletA = walletsA[walletIdxA];
            let walletB = walletsB[walletIdxB];
            let promise = Transfer(walletA, walletB, jettonPerTransfer, timeMeasure);
            transferPromises.push(promise);

            i++;
            walletIdxA++;
            walletIdxB++;

            if ((walletIdxA >= walletsA.length) || (walletIdxB >= walletsB.length)) {
                break;
            }
        }

        console.log(`Transfer jettons: runWithLimit (begin)...`);
        await runWithLimit(apiBatchSize, () => { return transferPromises.pop(); })
        console.log(`runWithLimit (end)...`);
    }

    console.table(timeMeasure.snapshot());
}

async function Transfer(
    walletFrom: Wallet,
    walletTo: Wallet,
    jettonPerTransfer: number,
    timeMeasure: TimeMeasure,
): Promise<void> {
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
    let tmUserExpId = timeMeasure.start("userExpTransferJettons");
    {
        console.log(`[Transfer]: sendJetton ${jettonPerTransfer} from '${walletFrom.getAddress()}' to '${walletTo.getAddress()}' (begin)...`);
        let tmId = timeMeasure.start("sendJetton");
        await walletFrom.sendJetton(
            "Test",
            walletTo.getAddress(),
            toJettons(jettonPerTransfer, JETTON_DECIMALS),
            toNano(FORWARD_TON),
            toNano('0.2'),  // Fee
            FORWARD_COMMENT
        );
        timeMeasure.stop(tmId);
        console.log(`[Transfer]: sendJetton ${jettonPerTransfer} from '${walletFrom.getAddress()}' to '${walletTo.getAddress()}' (end)...`);
    }

    // Wait for sender seqno update
    {
        let seqno = walletFrom.getSeqno();
        console.log(`[Transfer]: wait for sender seqno update '${walletFrom.getAddress()}' seqno = ${seqno} (begin)...`);
        let tmId = timeMeasure.start("waitForSeqNoChange");
        await walletFrom.waitForSeqNoChange(seqno);
        timeMeasure.stop(tmId);
        console.log(`[Transfer]: wait for sender seqno update '${walletFrom.getAddress()}' seqno = ${walletFrom.getSeqno()} (end)...`);
    }

    // Wait for receiver jetton balance update
    {
        console.log(`[Transfer]: wait for receiver '${walletTo.getAddress()}' jetton balance update = ${fromJettons(walletTo.getJettonBalance("Test"), JETTON_DECIMALS)} J (begin)...`);
        let tmId = timeMeasure.start("waitForJettonBalanceChange");
        await walletTo.waitForJettonBalanceChange("Test");
        timeMeasure.stop(tmId);
        timeMeasure.stop(tmUserExpId);
        console.log(`[Transfer]: wait for receiver '${walletTo.getAddress()}' jetton balance update = ${fromJettons(walletTo.getJettonBalance("Test"), JETTON_DECIMALS)} J (end)...`);
    }
}

run();
