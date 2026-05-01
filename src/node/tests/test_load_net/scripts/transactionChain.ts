import { Address, Message, Transaction, ExternalAddress } from '@ton/ton';
import { subtle } from 'crypto';
import { base64toHexStr, toHexPrefixed } from './utils';
import { ShardInfo, Wallet } from './wallet';
import { AppGlobals } from './globals';
import { GetBlockHeaderResponseOk, GetShardBlockProofResponseOk, LookupBlockResponseOk, TonCenterClient } from './tonCenterClient';

export type TransactionInfo = {
    tx: Transaction;
    shardInfo: ShardInfo;
    shardBlock: LookupBlockResponseOk;
    shardBlockProof: GetShardBlockProofResponseOk;
    masterBlock: GetBlockHeaderResponseOk;
}

interface MessageChain {
    transactions: Map<string, TransactionInfo>;
    messages: Map<string, Message>;
}

enum MessageDirection {
    In,
    Out
}

async function sha256(message: string): Promise<string> {
    const data = new TextEncoder().encode(message);
    const hashBuffer = await subtle.digest("SHA-256", data);
    return Array.from(new Uint8Array(hashBuffer))
        .map(b => b.toString(16).padStart(2, "0"))
        .join("");
}

async function messageHash(message: Message): Promise<string> {
    const msgHash = message.body.hash().toString('hex');
    const msgType = message.info.type;
    let msgLt = "";
    let addSrc = "";
    let addDst = "";

    switch (message.info.type) {
        case 'internal': {
            msgLt = message.info.createdLt.toString();
            addSrc = message.info.src.toRawString();
            addDst = message.info.dest.toRawString();
        } break;
        case 'external-in': {
            addDst = message.info.dest.toRawString();
        } break;
        case 'external-out': {
            addSrc = message.info.src.toRawString()
            msgLt = message.info.createdLt.toString();
        } break;
    }

    return sha256(msgHash + msgType + msgLt + addSrc + addDst);
}

export async function getCompleteMessageChain(
    address: Address,
    lastTxLt: bigint,
    lastTxHash: Buffer,
    txLimit: number,
    filterOutSessionChanges: boolean,
): Promise<MessageChain | undefined> {
    const allTransactions = new Map<string, TransactionInfo>();
    const allMessages = new Map<string, Message>();

    // Get transactions starting from the specific transaction
    let tonClient = (await AppGlobals.S()).nextTonCenterClient();
    const initialTx = await tonClient.getTransaction(address, lastTxLt.toString(), lastTxHash.toString("base64"));

    if (initialTx === null) {
        return {
            transactions: allTransactions,
            messages: allMessages
        };
    }

    // processMessage
    async function processMessage(msg: Message, messageDirection: MessageDirection, txLimit: number) {
        const msgHash = await messageHash(msg);

        if (allMessages.has(msgHash)) {
            return;
        }

        allMessages.set(msgHash, msg);
        let msgAddr: Address;
        let msgCreatedLtStr: string | undefined;

        switch (msg.info.type) {
            case 'internal': {
                if (messageDirection === MessageDirection.In) {
                    msgAddr = msg.info.src!;
                } else {
                    msgAddr = msg.info.dest!;
                }

                msgCreatedLtStr = msg.info.createdLt.toString();
            } break;
            case 'external-in': {
                msgAddr = msg.info.dest;
            } break;
            case 'external-out': {
                msgAddr = msg.info.src;
                msgCreatedLtStr = msg.info.createdLt.toString();
            } break;
        }

        const txs = await tonClient.getTransactions(msgAddr, { limit: txLimit, lt: msgCreatedLtStr });

        // Filter transactions that have this message in their messages
        for (const tx of txs) {
            let hasMessage = false;

            if (messageDirection === MessageDirection.In) {
                if (tx.outMessages != null) {
                    for (const outMsg of tx.outMessages.values()) {
                        const outMsgHash = await messageHash(outMsg);

                        if (msgHash === outMsgHash) {
                            hasMessage = true;
                            break;
                        }
                    }
                }
            } else { // messageDirection === MessageDirection.Out
                if (tx.inMessage != null) {
                    const inMsgHash = await messageHash(tx.inMessage!);
                    if (msgHash === inMsgHash) {
                        hasMessage = true;
                    }
                }
            }

            if (hasMessage) {
                let txInfo = await transactionInfo(tx, msgAddr.workChain);
                if (txInfo == null) {
                    return undefined;
                }

                await processTransaction(txInfo, txLimit);
            }
        }
    }

    // Process transactions recursively
    async function processTransaction(txInfo: TransactionInfo, txLimit: number) {
        const txHash = txInfo.tx.hash().toString('hex');

        if (allTransactions.has(txHash)) {
            return;
        }

        allTransactions.set(txHash, txInfo);

        // Process in_msg (incoming message)
        if (txInfo.tx.inMessage != null) {
            await processMessage(txInfo.tx.inMessage!, MessageDirection.In, txLimit);
        }

        // Process out_msgs (outgoing messages)
        if (txInfo.tx.outMessages != null) {
            for (const outMsg of txInfo.tx.outMessages.values()) {
                await processMessage(outMsg, MessageDirection.Out, txLimit);
            }
        }
    }

    async function transactionInfo(tx: Transaction, workChain: number): Promise<TransactionInfo | undefined> {
        if (tx.inMessage == null) {
            console.error("tx.inMessage == null");
            return undefined;
        }

        if (tx.inMessage.info.dest == null) {
            console.error("tx.inMessage.info.dest == null");
            return undefined;
        }

        if (tx.inMessage.info.dest instanceof ExternalAddress) {
            console.error("tx.inMessage.info.dest! instanceof Address");
            return undefined;
        }

        const shardInfo = Wallet.shardInfoForAddr(tx.inMessage.info.dest, 16);
        const shardBlock = await tonClient.lookupBlock({
            workchain: workChain,
            shard: shardInfo.shardIdInt,
            lt: tx.lt,
            timeoutMs: 15_000,
        });

        const shardBlockProof = await tonClient.getShardBlockProof({
            workchain: shardBlock.result.workchain,
            shard: shardBlock.result.shard,
            seqno: shardBlock.result.seqno,
            timeoutMs: 15_000,
        });

        const masterBlock = await tonClient.getBlockHeader({
            workchain: -1,
            shard: shardBlockProof.result.mc_id!.shard,
            seqno: shardBlockProof.result.mc_id!.seqno,
            root_hash: shardBlockProof.result.mc_id!.root_hash,
            file_hash: shardBlockProof.result.mc_id!.file_hash,
            timeoutMs: 15_000,
        });

        return {
            tx: tx,
            shardInfo: shardInfo,
            shardBlock: shardBlock,
            shardBlockProof: shardBlockProof,
            masterBlock: masterBlock,
        };
    }

    // Process all initial transactions
    let initialTxInfo = await transactionInfo(initialTx, address.workChain);
    if (initialTxInfo == null) {
        return undefined;
    }
    await processTransaction(initialTxInfo, txLimit);

    // 
    if (filterOutSessionChanges) {
        const [minCatchainSeqno, maxCatchainSeqno] = allTransactions.size
            ? [...allTransactions.values()].reduce(
                ([lo, hi], txInfo) => {
                    const x = txInfo.masterBlock.result.catchain_seqno!;
                    return [x < lo ? x : lo, x > hi ? x : hi]
                },
                [Infinity, -Infinity] as [number, number]
            )
            : [undefined, undefined] as const;

        if (minCatchainSeqno != maxCatchainSeqno) {
            return undefined;
        }
    }

    return {
        transactions: allTransactions,
        messages: allMessages
    };
}

async function printTransactionTree(
    tonClient: TonCenterClient,
    txId: string,
    chain: MessageChain,
    messageToConsumers: Map<string, string>,
    depth: number,
    rootTs: number | undefined,
    parentTs: number | undefined,
): Promise<number | undefined> {
    const txInfo = chain.transactions.get(txId);
    if (!txInfo) {
        return undefined;
    }

    if (txInfo.tx.inMessage == null) {
        console.error("tx.inMessage == null");
        return undefined;
    }

    if (txInfo.tx.inMessage.info.dest == null) {
        console.error("tx.inMessage.info.dest == null");
        return undefined;
    }

    if (txInfo.tx.inMessage.info.dest instanceof ExternalAddress) {
        console.error("tx.inMessage.info.dest! instanceof Address");
        return undefined;
    }

    // Print TX info
    let endUnixTs: number | undefined = 0;
    {
        let stepTime: string;
        if (parentTs != null) {
            stepTime = (txInfo.tx.now + 0.500 - parentTs).toFixed(3) + "±0.500";
        } else {
            stepTime = "-";
        }

        let shardBlockTime: string;
        if (rootTs != null) {
            shardBlockTime = (txInfo.tx.now + 0.500 - rootTs).toFixed(3) + "±0.500";
        } else {
            shardBlockTime = "-";
        }

        let masterBlockTime: string;
        if (rootTs != null) {
            masterBlockTime = (txInfo.masterBlock.result.gen_utime! + 0.500 - rootTs).toFixed(3) + "±0.500";
        } else {
            masterBlockTime = "-";
        }

        endUnixTs = txInfo.masterBlock.result.gen_utime!;

        console.log(`${"  ".repeat(depth * 2)}       [TX]        id: ${txId} | lt: ${txInfo.tx.lt} | now: ${txInfo.tx.now} | step: ${stepTime}s | shard: ${shardBlockTime}s | master: ${masterBlockTime}s`);
        if (txInfo.tx.description.type === "generic") {
            let desc = txInfo.tx.description;
            let computePhaseType = desc.computePhase.type;
            let success: boolean | undefined;

            if (desc.computePhase.type == "vm") {
                success = desc.computePhase.success;
            }

            console.log(`${"  ".repeat(depth * 2)}       [TX]      type: ${desc.type} | aborted: ${desc.aborted} | destroyed: ${desc.destroyed} | compute: ${computePhaseType} | vm success: ${success}}`);
        } else {
            console.log(`${"  ".repeat(depth * 2)}       [TX]      type: ${txInfo.tx.description.type}`);
        }
        console.log(`${"  ".repeat(depth * 2)}       [Account] ${txInfo.tx.inMessage.info.dest} | status: ${txInfo.tx.oldStatus}->${txInfo.tx.endStatus} | workchain: ${txInfo.shardInfo?.workchain} | shardId: ${toHexPrefixed(txInfo.shardInfo?.shardId)}, ${txInfo.shardInfo?.shardIdInt}`);

        if (txInfo.shardBlock.ok) {
            console.log(`${"  ".repeat(depth * 2)}       [Block]   seqno: ${txInfo.shardBlock.result.seqno} | root: ${base64toHexStr(txInfo.shardBlock.result.root_hash)} | file: ${base64toHexStr(txInfo.shardBlock.result.file_hash)}`);
            if (txInfo.masterBlock.ok) {
                console.log(`${"  ".repeat(depth * 2)}       [M Block] seqno: ${txInfo.masterBlock.result.id.seqno!} | root: ${base64toHexStr(txInfo.shardBlockProof.result.mc_id!.root_hash!)} | file: ${base64toHexStr(txInfo.shardBlockProof.result.mc_id!.file_hash!)}`);
                console.log(`${"  ".repeat(depth * 2)}       [M Block] gen_utime: ${txInfo.masterBlock.result.gen_utime!} | catchain_seqno: ${txInfo.masterBlock.result.catchain_seqno!}`);
            } else {
                console.log(`${"  ".repeat(depth * 2)}       [M Block] not found`);
            }
        } else {
            console.log(`${"  ".repeat(depth * 2)}       [Block]   not found`);
        }
    }

    console.log(`${"  ".repeat(depth * 2)}Input: [MSG]  Src: ${txInfo.tx.inMessage.info.src} ---> Dst: ${txInfo.tx.inMessage.info.dest}`);
    console.log(`${"  ".repeat(depth * 2)}       [MSG] Type: ${txInfo.tx.inMessage.info.type} | id: ${Wallet.messageIdToHex(txInfo.tx.inMessage)}`);
    console.log(`${"  ".repeat(depth * 2)}Outputs: ${txInfo.tx.outMessages.size}`);
    if (txInfo.tx.outMessages.size > 0) {
        let idx = 0;
        for (const outputMsg of txInfo.tx.outMessages.values()) {
            outputMsg.info.type
            console.log(`${"  ".repeat(depth * 2)}└───[${depth}.${idx}]  [MSG]  Src: ${outputMsg.info.src} ---> Dst: ${outputMsg.info.dest}`);
            console.log(`${"  ".repeat(depth * 2)}           [MSG] Type: ${outputMsg.info.type} | id: ${Wallet.messageIdToHex(outputMsg)}`);

            let outputMsgHash = await messageHash(outputMsg);
            const txId = messageToConsumers.get(outputMsgHash);
            if (txId != null) {
                const endUnixTsChild = await printTransactionTree(tonClient, txId, chain, messageToConsumers, depth + 1, rootTs ?? txInfo.tx.now, txInfo.tx.now);

                if (endUnixTs != null) {
                    if (endUnixTsChild != null) {
                        endUnixTs = Math.max(endUnixTs, endUnixTsChild);
                    } else {
                        // error
                        endUnixTs = undefined;
                    }
                }
            }
            idx++;
        }
    }

    return endUnixTs;
}

export async function printTransactionChain(chain: MessageChain, startTxTs: number | undefined): Promise<number | undefined> {
    console.log("Transactions Chain:");

    let tonClient = (await AppGlobals.S()).nextTonCenterClient();

    // Make `tree`
    const messageToConsumers = new Map<string/*out message hash*/, string/*in msg tx hash*/>();
    for (const [txId, txInfo] of chain.transactions) {
        if (txInfo.tx.inMessage != null) {
            let inMessageHash = await messageHash(txInfo.tx.inMessage);
            messageToConsumers.set(inMessageHash, txId);
        }
    }

    // Find root transactions
    const rootTransactions: string[] = [];
    for (const [txId, txInfo] of chain.transactions) {
        if ((txInfo.tx.inMessage == null) || (txInfo.tx.inMessage.info.src == null)) {
            rootTransactions.push(txId);
        }
    }

    // Print from each root
    var maxEndUnixTs: number | undefined = 0;
    for (const rootId of rootTransactions) {
        const endUnixTs = await printTransactionTree(tonClient, rootId, chain, messageToConsumers, 0, startTxTs, startTxTs);

        if (maxEndUnixTs != null) {
            if (endUnixTs != null) {
                maxEndUnixTs = Math.max(maxEndUnixTs, endUnixTs);
            } else {
                // error
                maxEndUnixTs = undefined;
            }
        }
    }

    return maxEndUnixTs;
}

/*
Take the address Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk and look up the transaction 64325205f7eddd9099c5e9fbf0ca1fbbf135e461727d99df562342f146a1d311:
{"id":"1","jsonrpc":"2.0","method":"getTransactions","params":{"address":"Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk","lt":"62355000003","hash":"64325205f7eddd9099c5e9fbf0ca1fbbf135e461727d99df562342f146a1d311","limit":1}}

Response:
{"ok":true,"result":[{"@type":"raw.transaction","address":{"@type":"accountAddress","account_address":"Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk"},"utime":1762797356,"data":"te6cckECCgEAAjwAA698rzi1TiTv7uzX0F5ib+p1I8OSOBKJlSl/pz3EwVmgHMAAAADoSlysOqnsRVo8P/O+kGDrsbjrJa+rp2Mxq/Pu5PjkTWpbp2bwAAAA6EaMHDaRInLAADQIAQIDAgHgBAUAgnIRSaK4B/MGTv/y2QsBn6QkVAb4YKDW5V4FvbigQHTHkFgv1aAdtvLJI1N5KcSAvY7iOfoC1nqrtt6tw/dCykBMAgcMBgRACAkB34n/lecWqcSd/d2a+gvMTf1OpHhyRwJRMqUv9Oe4mCs0A5gFJ1lTlcS1/fpRrKAsTPXicV/PS9yiPoySCXe39jJuKzRSp00XytovwnvQaMbQCYisXQSAHOI/kEl6FR0sJ2IAEAAAAVNIkTtAAAAAKAwGAQHfBwBqQgBsnT5LwW+251RM0XADLzmTXSH/c5OgnJDB0EoDVagwFag34R1gAAAAAAAAAAAAAAAAAAAArUn/lecWqcSd/d2a+gvMTf1OpHhyRwJRMqUv9Oe4mCs0A5kANk6fJeC323OqJmi4AZecya6Q/7nJ0E5IYOglAarUGArUG/COsAAAAAAAHQlLlYjSJE5YQAClQXZQELB2AxOIAAAAAAAAAAAQgAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAACAAYcAAAAAAAAIAAAAAAANCgxyf9BPqeGgQHcSUpljidRCNY9Z5iPYOqJQbD/jA8EBQFYwzYJQq","transaction_id":{"@type":"internal.transactionId","lt":"62355000003","hash":"ZDJSBfft3ZCZxen78Mofu/E15GFyfZnfViNC8Uah0xE="},"fee":"0","storage_fee":"0","other_fee":"0","in_msg":{"@type":"raw.message","hash":"iIRuSn5rTit7Byfv/gUkH+K5wNaMX1GLTthavSoa7Zc=","source":"","destination":"Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk","value":"0","extra_currencies":[],"fwd_fee":"0","ihr_fee":"0","created_lt":"0","body_hash":"NQeD2SsYOdhIz8qKrNRkaivhn0w4tjoeCuyxwWag66A=","msg_data":{"@type":"msg.dataRaw","body":"te6cckEBAgEAhwABmqTrKnK4lr+/SjWUBYmevE4r+el7lEfRkkEu9v7GTcVmilTpovlbRfhPeg0Y2gExFYugkAOcR/IJL0KjpYTsQAIAAAAqaRInaAAAAAUBAQBqQgBsnT5LwW+251RM0XADLzmTXSH/c5OgnJDB0EoDVagwFag34R1gAAAAAAAAAAAAAAAAAACda51K","init_state":""},"message":"pOsqcriWv79KNZQFiZ68Tiv56XuUR9GSQS72/sZNxWaKVOmi+VtF+E96DRjaATEVi6CQA5xH8gkv\nQqOlhOxAAgAAACppEidoAAAABQE=\n"},"out_msgs":[{"@type":"raw.message","hash":"uoN1xZt/SlSSjHzBeq7aNZHzeL3RveG+5FuGVKJlgeY=","source":"Ef_K84tU4k7-7s19BeYm_qdSPDkjgSiZUpf6c9xMFZoBzGbk","destination":"EQDZOnyXgt9tzqiZouAGXnMmukP-5ydBOSGDoJQGq1BgKx2y","value":"30000000000","extra_currencies":[],"fwd_fee":"0","ihr_fee":"0","created_lt":"62355000004","body_hash":"lqKW0iTyhcZ77pPDD4owkVfw2qNdxbh+QQt4YwoJz8c=","msg_data":{"@type":"msg.dataRaw","body":"te6cckEBAQEAAgAAAEysuc0=","init_state":""},"message":""}]}],"jsonrpc":"2.0","id":"1"}

Look up the block:
/lookupBlock?workchain=0&shard=-4035225266123964416&lt=62355000003

Response:
{"ok":true,"result":{"@type":"ton.blockIdExt","workchain":0,"shard":"-2305843009213693952","seqno":19783,"root_hash":"Vr/Jjyl2CnN68Rl0HpHxVOEq9E3fGS3HGfBx87BI6W0=","file_hash":"8+0/Q8I/TVTmquoFsRmO4l1uBzaO2B/HQ3jYGHr5+F8=","@extra":"1762802307.2277544:2:0.23902926815413095"}}

Look up the shard proof:
/getShardBlockProof?workchain=0&shard=-2305843009213693952&seqno=19783

Response:
..."mc_id":{"@type":"ton.blockIdExt","workchain":-1,"shard":"-9223372036854775808","seqno":49345,"root_hash":"F8krddIymchlD6iXFP92hzgczT4nB24On3RW7lav/8E=","file_hash":"VM+/xa3yXzf9veXRm8kT243ke60sx73ZyWWEd21p2mY="},"links":[{"@type":"blocks.shardBlockLink","id":{"@type":"ton.blockIdExt","workchain":0,"shard":"-2305843009213693952","seqno":19785,"root_hash":"bLyR+KJoqWTH+BNnTcQChDWMyDf/hEZvfzh8mo0Gp0Q=","file_hash":"SjNF6VtJpRKFSQcHYZfna0WAhCdto5q5MHSnXCx0Q/g="}.........

We can see that "seqno":49345

Look up the master block:
/getBlockHeader?workchain=-1&shard=-9223372036854775808&seqno=49345&root_hash=F8krddIymchlD6iXFP92hzgczT4nB24On3RW7lav%2F8E%3D&file_hash=VM%2B%2Fxa3yXzf9veXRm8kT243ke60sx73ZyWWEd21p2mY%3D




*/