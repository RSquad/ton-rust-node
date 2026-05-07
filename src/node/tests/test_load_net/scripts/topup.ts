import { TonClient } from "@ton/ton";
import { Address, fromNano, internal, SendMode, toNano } from "@ton/core";
import { checkEnvs, input } from "./utils";
import { WalletContractV3R2 } from "@ton/ton";

async function run() {
    if (process.argv.length !== 4) {
        console.error("Usage: bun scripts/nodectl-topup-wallets.ts <address> <amount>");
        console.log("Set the environment variables [MASTER_WALLET_KEY, API_ENDPOINTS] and try again");
        process.exit(1);
    }

    checkEnvs(["MASTER_WALLET_KEY", "API_ENDPOINTS"]);

    const masterKey = Buffer.from(process.env.MASTER_WALLET_KEY!, "hex");
    if (masterKey.length !== 64) {
        throw new Error("MASTER_WALLET_KEY must be 64 bytes");
    }
    const masterWallet = WalletContractV3R2.create({
        workchain: -1,
        publicKey: masterKey.subarray(32),
        walletId: 42
    });
    console.log(`Master wallet address: ${masterWallet.address}`);

    let address;
    try {
        address = Address.parse(process.argv[2]);
    } catch (error) {
        console.error(`Invalid address: ${process.argv[2]}, ${error}`);
        process.exit(1);
    }

    let amount;
    try {
        amount = toNano(process.argv[3]);
    } catch (error) {
        console.error(`Invalid amount: ${process.argv[3]}, ${error}`);
        process.exit(1);
    }

    const tonClient = new TonClient({
        endpoint: process.env.API_ENDPOINTS!.split(",")[0] + "jsonRPC",
    });
    const master = tonClient.open(masterWallet);
    const balanceBefore = await tonClient.getBalance(address);

    await master.sendTransfer({
        seqno: await master.getSeqno(),
        secretKey: masterKey,
        messages: [
            internal({
                to: address,
                value: amount,
                bounce: false,
            }),
        ],
        sendMode: SendMode.PAY_GAS_SEPARATELY,
    });
    console.log(`Sent ${fromNano(amount)} TON to ${address}, waiting for balance update...`);

    const timeout = 60_000;
    const poll = 1_000;
    const start = Date.now();
    while (Date.now() - start < timeout) {
        const balance = await tonClient.getBalance(address);
        if (balance > balanceBefore) {
            return;
        }
        await new Promise((r) => setTimeout(r, poll));
    }
    throw new Error(`Timed out waiting for balance update after ${timeout / 1000}s`);
}

(async () => {
    try {
        await run();
    } catch (error) {
        console.error(`Error: ${error}`);
        process.exit(1);
    }
})();
