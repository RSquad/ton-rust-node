import { Address } from "@ton/ton";
import { getCompleteMessageChain, printTransactionChain } from "./transactionChain";
import { parseArgs } from 'util';
import { hexToBase64Str } from "./utils";
import { AppGlobals } from "./globals";

/**
 * Example: bun run ./scripts/getTransactionChain.ts --addr="EQDNRGAhJs2GAeiXagdj5XRlmXkU0RP_OQmwbHkt5w4eJ4Xz" --lt=1184483000001 --hash="dd70eef6880464faf3a17a2abf24478aa39f955bce05ba698f691812aa342486" --filter_out_session_changes=1
 */

export async function run() {
    const { values, positionals } = parseArgs({
        options: {
            addr: { type: 'string', short: 'a' },
            lt: { type: 'string', short: 'l' },
            hash: { type: 'string', short: 'h' },
            filter_out_session_changes: { type: 'string', short: 's' },
        },
        allowPositionals: true,
    });

    const tonClient = (await AppGlobals.S()).nextTonCenterClient();

    const address = Address.parse(values.addr!);
    const lt = values.lt!;
    const hash = values.hash!;
    const filterOutSessionChanges = Number.parseInt(values.filter_out_session_changes ?? "0") > 0;

    let tx = await tonClient.getTransaction(address, lt, hexToBase64Str(hash));
    if (tx == null) {
        console.error(`Transaction not found: address: ${address}, lt: ${lt}, hash: ${hash}`);
        return;
    }

    console.log(`Fetch transactions chain...`);
    const chain = await getCompleteMessageChain(address, tx.lt, tx.hash(), 100, filterOutSessionChanges);

    if (chain == null) {
        return;
    }

    console.log(`Found ${chain.transactions.size} transactions`);
    console.log(`Found ${chain.messages.size} messages`);

    await printTransactionChain(chain, undefined);
}

run();