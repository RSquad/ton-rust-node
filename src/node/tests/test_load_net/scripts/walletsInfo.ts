import { Address } from "@ton/core";
import { runWithLimit } from "./batchedPromise";
import { AppGlobals } from "./globals";
import { fromJettons, fromTons } from "./utils";

const JETTON_MINTER = Address.parse(process.env.JETTON_MINTER!);
const JETTON_DECIMALS = 9;

export async function run() {
    const apiBatchSize = (await AppGlobals.S()).getApiBatchSize();
    let walletStorage = (await AppGlobals.S()).getWalletStorage();
    let wallets = await walletStorage.readAll();

    // Update ton balance
    {
        let promises: Promise<bigint>[] = [];

        for (let wallet of wallets) {
            promises.push(wallet.updateBalance());
        }

        console.log(`Update ton balances: runWithLimit (begin)...`);
        await runWithLimit(apiBatchSize, () => { return promises.pop(); })
        console.log(`runWithLimit (end)...`);
    }

    // Set jetton minter address
    {
        let promises: Promise<void>[] = [];

        for (let wallet of wallets) {
            promises.push(wallet.setJettonMinter("Test", JETTON_MINTER));
        }

        console.log(`Set jetton minter address: runWithLimit (begin)...`);
        await runWithLimit(apiBatchSize, () => { return promises.pop(); })
        console.log(`runWithLimit (end)...`);
    }

    // Update jetton balance
    {
        let promises: Promise<bigint>[] = [];

        for (let wallet of wallets) {
            promises.push(wallet.updateJettonBalance("Test"));
        }

        console.log(`Update jetton balances: runWithLimit (begin)...`);
        await runWithLimit(apiBatchSize, () => { return promises.pop(); })
        console.log(`runWithLimit (end)...`);
    }

    // Update seqno
    {
        let promises: Promise<number>[] = [];

        for (let wallet of wallets) {
            promises.push(wallet.updateSeqno());
        }

        console.log(`Update seqno: runWithLimit (begin)...`);
        await runWithLimit(apiBatchSize, () => { return promises.pop(); })
        console.log(`runWithLimit (end)...`);
    }

    // Info
    for (let wallet of wallets) {
        console.log(`'${wallet.getAddress()}', TON: ${fromTons(wallet.getBalance())}, J: ${fromJettons(wallet.getJettonBalance("Test"), JETTON_DECIMALS)}, seqno: ${wallet.getSeqno()}`);
    }

    console.log(`\nTotal wallets count: ${wallets.length}`);
}

run();