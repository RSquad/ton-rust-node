import { Address, toNano } from '@ton/core';
import { parseArgs } from 'util';
import { AppGlobals } from './globals';
import { Wallet, WalletVersion } from './wallet';
import { WalletUtils } from './walletUtils';
import { findJettonNotifiesForAddress, JettonTransferNotify } from './filterJettonTransferNotification';
import { Statistics } from './statistics';
import { runWithLimit } from './batchedPromise';
import { getCompleteMessageChain, printTransactionChain } from './transactionChain';
import { hexToBase64Str, toJettons } from './utils';
import { sleep } from '@ton/blueprint';
import { TimeMeasure } from './timeMeasure';

const JETTON_MINTER = Address.parse(process.env.JETTON_MINTER!);
const JETTON_DECIMALS = 9;
const FORWARD_TON = 0.01;
const FORWARD_COMMENT = 'test jetton';

/**
 * Example: bun run ./scripts/transferJettonSeq.ts --count 5 --tons 5 --jettons 50 --stat_level 1 --filter_out_session_changes 1
 */

export async function run() {
    const { values, positionals } = parseArgs({
        options: {
            count: { type: 'string', short: 'c' },
            tons: { type: 'string', short: 't' },
            jettons: { type: 'string', short: 'j' },
            stat_level: { type: 'string', short: 's' },
            filter_out_session_changes: { type: 'string', short: 's' },
        },
        allowPositionals: true,
    });

    let faucetWallet = (await AppGlobals.S()).getFaucetWallet();
    console.log(`Faucet wallet: '${faucetWallet.getAddress()}'`);

    const walletsCount = Number.parseInt(values.count!);
    const tonAmountPerWallet = Number.parseFloat(values.tons ?? "5.0");
    const jettonAmountPerWallet = Number.parseFloat(values.jettons ?? "50.0");
    const statLevel = Number.parseInt(values.stat_level ?? "0");
    const filterOutSessionChanges = Number.parseInt(values.filter_out_session_changes ?? "0") > 0;

    console.log(`Generate new V3R2 wallets (${walletsCount})...`)
    let wallets = await WalletUtils.generate(walletsCount, WalletVersion.V3R2, undefined, undefined);

    console.log(`Faucet wallets (${walletsCount})...`)
    await faucetWallets(faucetWallet, wallets, tonAmountPerWallet, jettonAmountPerWallet, statLevel, filterOutSessionChanges);
}

async function faucetWallets(
    faucetWallet: Wallet,
    wallets: Wallet[],
    tonAmountPerWallet: number,
    jettonAmountPerWallet: number,
    statLevel: number,
    filterOutSessionChanges: boolean,
) {
    const apiBatchSize = (await AppGlobals.S()).getApiBatchSize();
    let userExpTimeMeasure = new TimeMeasure({ keepSamples: true, maxSamplesPerName: 10000 });

    // Faucet TONs (faucet -> wallets)
    {
        console.log(`-------- Faucet wallets (begin) --------`);
        await faucetWallet.updateSeqno();

        for (let wallet of wallets) {
            console.log(`Send ${tonAmountPerWallet} TON from Faucet '${faucetWallet.getAddress()}' to Wallet: '${wallet.getAddress()}'...`);
            console.log(`faucetSeqno = ${faucetWallet.getSeqno()}`);

            let tmId = userExpTimeMeasure.start("userExpSendTon");
            let seqno = faucetWallet.getSeqno();
            let extInMsg = await faucetWallet.sendTon(
                wallet.getAddress(),
                toNano(tonAmountPerWallet),
                wallet.getStateInit(),
            );

            console.log(`message id: ${Wallet.messageIdToHex(extInMsg)}`);
            console.log(`message base64: ${Wallet.extInMessageToBase64(extInMsg)}`);
            console.log(`Waiting...`);
            await Promise.all([
                faucetWallet.waitForSeqNoChange(seqno),
                wallet.waitForBalanceChange()
            ]);
            userExpTimeMeasure.stop(tmId);
        }

        console.log(`-------- Faucet wallets (end) --------`);
    }

    // Faucet JetTONs
    let jettonTxTs: Map<Address, [Wallet, number]> = new Map();
    {
        console.log(`-------- Faucet JetTONs (begin) --------`);

        await Promise.all([
            await faucetWallet.updateSeqno(),
            await faucetWallet.setJettonMinter("Test", JETTON_MINTER)
        ]);

        console.log(`faucetJettonWallet = ${faucetWallet.getJettonWalletAddr("Test")}`);

        {
            console.log(`--- Update wallets jetton address (begin) ---`);

            let promises: Promise<void>[] = [];
            for (let wallet of wallets) {
                promises.push(wallet.setJettonMinter("Test", JETTON_MINTER));
            }
            console.log(`runWithLimit (begin)...`);
            await runWithLimit(apiBatchSize, () => { return promises.pop(); })
            console.log(`runWithLimit (end)...`);

            console.log(`--- Update wallets jetton address (end) ---`);
        }

        {
            console.log(`--- Send Jettons (begin) ---`);

            for (let wallet of wallets) {
                console.log(`Send ${jettonAmountPerWallet} Jettons('Test') and ${FORWARD_TON} TON from Faucet '${faucetWallet.getJettonWalletAddr("Test")}' to Wallet: '${wallet.getJettonWalletAddr("Test")}'...`);
                console.log(`walletSeqno = ${wallet.getSeqno()}`);
                console.log(`faucetSeqno = ${faucetWallet.getSeqno()}`);

                let tmId = userExpTimeMeasure.start("userExpSendJetton");
                const nowTs = Date.now() / 1000.0;
                let seqno = faucetWallet.getSeqno();
                let extInMsg = await faucetWallet.sendJetton(
                    "Test",
                    wallet.getAddress(),
                    toJettons(jettonAmountPerWallet, JETTON_DECIMALS),
                    toNano(FORWARD_TON),
                    toNano('0.5'),  // Fee
                    FORWARD_COMMENT
                );
                console.log(`message id: ${Wallet.messageIdToHex(extInMsg)}`);
                console.log(`message base64: ${Wallet.extInMessageToBase64(extInMsg)}`);

                jettonTxTs.set(wallet.getAddress(), [wallet, nowTs]);

                console.log(`Waiting...`);
                await Promise.all([
                    faucetWallet.waitForSeqNoChange(seqno),
                    wallet.waitForJettonBalanceChange("Test"),
                ]);
                userExpTimeMeasure.stop(tmId);
            }

            let walletStorage = (await AppGlobals.S()).getWalletStorage();
            await walletStorage.append(Array.from(wallets.values())).finally(() => {
                walletStorage.flush();
            });

            console.log(`--- Send Jettons (end) ---`);
        }

        console.log(`-------- Faucet JetTONs (end) --------`);
    }

    // Statistics
    if (statLevel > 0) {
        console.log(`-------- Statistics (begin) --------`);

        if (statLevel > 1) {
            const results: JettonTransferNotify[] = [];

            for (let [walletAddr, [wallet, startTxTs]] of jettonTxTs.entries()) {
                let jettonNotifies: JettonTransferNotify[] = [];

                while (jettonNotifies.length == 0) {
                    jettonNotifies = await findJettonNotifiesForAddress(walletAddr, startTxTs, 200);
                    console.log(`jettonNotifies.length = ${jettonNotifies.length}`);
                    if (jettonNotifies.length == 0) {
                        await sleep(500);
                    }
                }

                // update endUnixTs
                let updatedJettonNotifies: JettonTransferNotify[];

                console.log(`runWithLimit (begin)...`);

                let promises = jettonNotifies.map(async (m) => {
                    console.log(`getTransaction: addr='${Address.parse(m.toAddr)}', lt=${m.lt}, tx='${hexToBase64Str(m.txHash)}'...`);

                    let tx = await (await AppGlobals.S()).nextTonCenterClient().getTransaction(Address.parse(m.toAddr), m.lt, hexToBase64Str(m.txHash));
                    if (tx == null) {
                        const errMsg = `Transaction not found: address: ${m.toAddr}, lt: ${m.lt}, hash: ${m.txHash}`;
                        console.error(errMsg);
                        throw Error(errMsg);
                    }

                    console.log(`Fetch transactions chain for TX id: ${tx.hash().toString("hex")}...`);
                    let addressTo = tx.inMessage!.info.dest;

                    if (addressTo instanceof Address) {
                        const chain = await getCompleteMessageChain(addressTo, tx.lt, tx.hash(), 200, filterOutSessionChanges);

                        if (chain != null) {
                            console.log(`Found ${chain.transactions.size} transactions`);
                            console.log(`Found ${chain.messages.size} messages`);

                            const endUnixTs = await printTransactionChain(chain, startTxTs);
                            m.endUnixTs = endUnixTs;
                        } else {
                            return undefined;
                        }
                    }

                    return m;
                });

                updatedJettonNotifies = (await runWithLimit(apiBatchSize, () => { return promises.pop(); })).results.filter(
                    (x): x is JettonTransferNotify => x !== undefined
                );
                console.log(`runWithLimit (end)...`);

                results.push(...updatedJettonNotifies);
            }

            console.log(`Found ${results.length} Jetton Notify txs:`);

            // filter out errors from results
            const resultsNoErr = results.filter(x => { return (x.endUnixTs != null); });

            console.log(`Found ${resultsNoErr.length} Jetton Notify txs without errors`);

            if (results.length > resultsNoErr.length) {
                console.error('There are errors!');
            }

            let satistics = new Statistics();

            console.table(
                resultsNoErr.map(r => {
                    satistics.addEvent(r.beginUnixTs, r.endUnixTs! + 0.500, 0.5);

                    return {
                        recipientJettonWallet: r.toAddr,
                        lt: r.lt,
                        beginUnixTs: r.beginUnixTs.toFixed(3),
                        endUnixTs: `${(r.endUnixTs! + 0.500).toFixed(3)}±0.500`,
                        delta: `${(r.endUnixTs! + 0.500 - r.beginUnixTs).toFixed(3)}±0.500`,
                        txHash: r.txHash,
                    };
                })
            );

            {
                console.log(`\nNode Transaction Statistics:`);
                const s = satistics.summarize();
                console.log(`Transfers count: ${s.count}`);
                console.table(s.stat);
            }
        }

        {
            console.log(`\nUser Experience Statistics:`);
            console.table(userExpTimeMeasure.snapshot());
        }

        {
            const tm = (await AppGlobals.S()).getTimeMeasure();
            console.log(`\nJSON-RPC Statistics:`);
            console.table(tm.snapshot());
        }

        console.log(`--------Statistics(end) --------`);
    }
}

run();
