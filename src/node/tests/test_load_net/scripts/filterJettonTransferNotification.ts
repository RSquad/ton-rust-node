import { Address, Cell, Slice, beginCell } from "@ton/ton";
import { AppGlobals } from "./globals";

const JETTON_NOTIFY = 0x7362d09c as const;

function readOp(body?: Cell | null): number | null {
    if (!body) return null;
    const s = body.beginParse();
    if (s.remainingBits < 32) return null;
    return Number(s.loadUint(32));
}

function parseJettonNotify(body: Cell) {
    const s: Slice = body.beginParse();
    const op = Number(s.loadUint(32));
    if (op !== JETTON_NOTIFY) {
        throw new Error(`Not a jetton transfer_notification (got 0x${op.toString(16)})`);
    }

    const queryId = s.loadUintBig(64);
    const amount = s.loadCoins();
    const sender = s.loadMaybeAddress();

    const eitherTag = s.loadBit();
    let forwardPayload: Cell;

    if (eitherTag === false) {
        const builder = beginCell();
        if (s.remainingBits > 0) {
            builder.storeSlice(s);
        }

        forwardPayload = builder.endCell();
    } else {
        forwardPayload = s.loadRef();
    }

    let forwardComment: string | undefined;
    try {
        const ps = forwardPayload.beginParse();
        if (ps.remainingBits >= 32 && Number(ps.preloadUint(32)) === 0) {
            ps.loadUint(32); // comment op = 0
            forwardComment = ps.loadStringRefTail();
        }
    } catch {
        // ignore
    }

    return {
        op,
        queryId,
        amount: amount.toString(),
        sender: sender?.toString() ?? null,
        forwardPayload,
        forwardComment,
        eitherTag,
    };
}

export type JettonTransferNotify = {
    txAddr: string;
    txHash: string;
    lt: string;
    beginUnixTs: number;
    endUnixTs: number | undefined;
    fromAddr: string;
    toAddr: string;
};

export async function findJettonNotifiesForAddress(address: Address, beginUnixTs: number, limitTx: number): Promise<JettonTransferNotify[]> {
    const tonClient = (await AppGlobals.S()).nextTonCenterClient();

    let matches: JettonTransferNotify[] = [];

    // You can paginate with {lt, hash} if you need to go deeper.
    const txs = await tonClient.getTransactions(address, { limit: limitTx });

    for (const tx of txs) {
        const inMsg = tx.inMessage;
        if (!inMsg || inMsg.info.type !== "internal") {
            continue;
        }

        const op = readOp(inMsg.body);
        if (op !== JETTON_NOTIFY) {
            continue;
        }

        // Try to parse payload details (amount, sender wallet, etc.)
        let parsed: ReturnType<typeof parseJettonNotify> | undefined;
        try {
            parsed = parseJettonNotify(inMsg.body!);
        } catch (e) {
            console.log(e);
        }

        matches.push({
            txAddr: tx.address.toString(),
            txHash: tx.hash().toString("hex"),
            lt: tx.lt.toString(),
            beginUnixTs: beginUnixTs,
            endUnixTs: tx.now,
            fromAddr: inMsg.info.src?.toString() ?? "",
            toAddr: inMsg.info.dest?.toString() ?? "",
        });
    }

    return matches;
}
