import { Cell, loadShardIdent } from "@ton/core";
import { LiteClient, LiteRoundRobinEngine, LiteSingleEngine } from "ton-lite-client";
import { program } from "commander";

// ---------------- ENV VALIDATION ----------------
const PUBLIC_KEY = process.env.LITESERVER_PUBLIC_KEY;
const NETWORK_NAME = process.env.NETWORK_NAME;
const LITESERVER_HOST = process.env.LITESERVER_HOST;
const LITESERVER_PORT = Number(process.env.LITESERVER_PORT);

const envErrors: string[] = [];
if (!LITESERVER_HOST) envErrors.push("LITESERVER_HOST is not set");
if (!process.env.LITESERVER_PORT || isNaN(LITESERVER_PORT)) envErrors.push("LITESERVER_PORT is not set or not a number");
if (!PUBLIC_KEY) {
    envErrors.push("LITESERVER_PUBLIC_KEY is not set");
} else if (!/^[0-9a-fA-F]{64}$/.test(PUBLIC_KEY)) {
    envErrors.push("LITESERVER_PUBLIC_KEY must be a 64-character hex string (32 bytes)");
}

if (envErrors.length > 0) {
    console.error("❌ Missing or invalid environment variables:");
    envErrors.forEach(e => console.error(`   - ${e}`));
    console.error("\nMake sure your .env file contains LITESERVER_HOST, LITESERVER_PORT, LITESERVER_PUBLIC_KEY");
    process.exit(1);
}

// ---------------- CLI ARGS ----------------
program
    .name("ton-tps")
    .description("Count TON transactions per second across all shards")
    .requiredOption("-s, --start <seqno>", "start masterchain seqno", Number)
    .requiredOption("-e, --end <seqno>", "end masterchain seqno", Number)
    .option("-p, --page <size>", "transaction page size", Number, 1024)
    .parse(process.argv);

const opts = program.opts();
const START_SEQNO: number = opts.start;
const END_SEQNO: number = opts.end;
const TX_PAGE: number = opts.page;

const cliErrors: string[] = [];
if (!isNaN(START_SEQNO) && START_SEQNO < 1) cliErrors.push("--start must be >= 1 (seqno - 1 is used for time baseline)");
if (isNaN(END_SEQNO)) cliErrors.push("--end must be a number");
if (isNaN(TX_PAGE) || TX_PAGE < 1) cliErrors.push("--page must be a positive number");
if (!isNaN(END_SEQNO) && END_SEQNO < 0) cliErrors.push("--end must be non-negative");
if (!isNaN(START_SEQNO) && !isNaN(END_SEQNO) && START_SEQNO > END_SEQNO) {
    cliErrors.push("--start must be <= --end");
}

if (cliErrors.length > 0) {
    console.error("❌ Invalid arguments:");
    cliErrors.forEach(e => console.error(`   - ${e}`));
    process.exit(1);
}
// ----------------------------------------

type BlockID = {
    workchain: number;
    shard: string;
    seqno: number;
    rootHash: Buffer;
    fileHash: Buffer;
};

function createClient() {
    const engine = new LiteRoundRobinEngine([
        new LiteSingleEngine({
            host: `tcp://${LITESERVER_HOST}:${LITESERVER_PORT}`,
            publicKey: Buffer.from(PUBLIC_KEY, "hex"),
        }),
    ]);

    return new LiteClient({ engine });
}

async function getMasterchainBlockTime(client: LiteClient, seqno: number): Promise<number> {
    const res = await client.lookupBlockByID({
        workchain: -1,
        shard: "-9223372036854775808",
        seqno
    });

    const proofRoot = Cell.fromBoc(res.headerProof)[0];
    const blockCell = proofRoot.isExotic ? proofRoot.refs[0] : proofRoot;

    const b = blockCell.beginParse();
    const blockTag = b.loadUint(32); // block#11ef55aa
    if (blockTag !== 0x11ef55aa) {
        throw new Error(`Unexpected Block tag: 0x${blockTag.toString(16)}`);
    }

    b.loadInt(32); // global_id

    // info:^BlockInfo (ref)
    const infoCell = b.loadRef();
    const i = infoCell.beginParse();

    // ---- Parse BlockInfo ----
    const infoTag = i.loadUint(32); // block_info#9bc7a987
    if (infoTag !== 0x9bc7a987) {
        throw new Error(`Unexpected BlockInfo tag: 0x${infoTag.toString(16)}`);
    }

    i.loadUint(32); // version

    i.loadBit();    // not_master
    i.loadBit();                      // after_merge
    i.loadBit();                      // before_split
    i.loadBit();                      // after_split
    i.loadBit();                      // want_split (Bool)
    i.loadBit();                      // want_merge (Bool)
    i.loadBit();                      // key_block (Bool)
    i.loadBit();                      // vert_seqno_incr
    i.loadUint(8);                    // flags

    i.loadUint(32); // seq_no
    i.loadUint(32); // vert_seq_no

    // after_merge only affects prev_ref (a reference cell), not inline data
    loadShardIdent(i); // shard:ShardIdent
    const genUtime = i.loadUint(32); // gen_utime:uint32
    return genUtime;
}

type ShardsInfo = {
    shards: Record<string, Record<string, number>>;
};

function extractShardBlocks(shardsInfo: ShardsInfo) {
    const result: { workchain: number; shard: string; seqno: number }[] = [];

    if (
        !shardsInfo ||
        typeof shardsInfo !== "object" ||
        !("shards" in shardsInfo) ||
        typeof (shardsInfo as any).shards !== "object"
    ) {
        throw new Error("Unexpected shardsInfo shape: missing or invalid 'shards' field");
    }

    const { shards } = shardsInfo as ShardsInfo;

    for (const wc of Object.keys(shards)) {
        if (wc === "-1") continue;

        const wcShards = shards[wc];
        if (typeof wcShards !== "object") {
            throw new Error(`Unexpected shard data for workchain ${wc}`);
        }

        for (const shardId of Object.keys(wcShards)) {
            const seqno = wcShards[shardId];
            if (typeof seqno !== "number") {
                throw new Error(`Unexpected seqno type for shard ${wc}:${shardId}: ${typeof seqno}`);
            }
            result.push({
                workchain: Number(wc),
                shard: shardId,
                seqno
            });
        }
    }

    return result;
}

async function countShardTransactions(client: LiteClient, block: BlockID): Promise<number> {
    let total = 0;
    let after: { account: Buffer; lt: string } | null = null;

    while (true) {
        const mode = 1 + 2 + 4 + (after ? 128 : 0);
        const res = await client.listBlockTransactions(
            block,
            {
                mode,
                count: TX_PAGE,
                after: after ?? undefined
            }
        );

        total += res.ids.length;

        if (!res.incomplete) break;

        const last = res.ids[res.ids.length - 1];
        if (!last?.account || !last?.lt) break;
        after = { account: last.account, lt: last.lt };
    }

    return total;
}

async function processSeqno(
    client: LiteClient,
    seqno: number,
    seenShards: Set<string>,
    shardStats: Map<string, { blocks: number; tx: number }>
): Promise<{ mc: number; shard: number }> {
    const mcHeader = await client.lookupBlockByID({
        workchain: -1,
        shard: "-9223372036854775808",
        seqno
    });
    const mcBlock: BlockID = {
        workchain: mcHeader.id.workchain,
        shard: mcHeader.id.shard,
        seqno: mcHeader.id.seqno,
        rootHash: mcHeader.id.rootHash,
        fileHash: mcHeader.id.fileHash
    };

    const mcTx = await countShardTransactions(client, mcBlock);
    console.log(`  masterchain seqno ${mcBlock.seqno} → ${mcTx} tx`);

    // track MC stats
    const mcKey = `${mcBlock.workchain}:${mcBlock.shard}`;
    const mcStat = shardStats.get(mcKey) ?? { blocks: 0, tx: 0 };
    mcStat.blocks++;
    mcStat.tx += mcTx;
    shardStats.set(mcKey, mcStat);

    const shardsInfo = await client.getAllShardsInfo(mcBlock);
    const shardBlocks = extractShardBlocks(shardsInfo);

    const newShards = shardBlocks.filter(shard => {
        const key = `${shard.workchain}:${shard.shard}:${shard.seqno}`;
        if (seenShards.has(key)) {
            console.log(`  shard ${shard.shard} seqno ${shard.seqno} → SKIPPED (already counted)`);
            return false;
        }
        seenShards.add(key);
        return true;
    });

    const txCounts = await Promise.all(
        newShards.map(async (shard) => {
            const shardHeader = await client.lookupBlockByID(shard);
            const shardBlock: BlockID = {
                workchain: shardHeader.id.workchain,
                shard: shardHeader.id.shard,
                seqno: shardHeader.id.seqno,
                rootHash: shardHeader.id.rootHash,
                fileHash: shardHeader.id.fileHash
            };
            const tx = await countShardTransactions(client, shardBlock);
            console.log(
                `  shard ${shardBlock.shard} seqno ${shardBlock.seqno} → ${tx} tx`
            );

            // track shard stats
            const shardKey = `${shard.workchain}:${shard.shard}`;
            const stat = shardStats.get(shardKey) ?? { blocks: 0, tx: 0 };
            stat.blocks++;
            stat.tx += tx;
            shardStats.set(shardKey, stat);

            return tx;
        })
    );

    const shardTx = txCounts.reduce((sum, tx) => sum + tx, 0);
    return { mc: mcTx, shard: shardTx };
}
async function main() {
    console.log(`LITESERVER:  ${LITESERVER_HOST}:${LITESERVER_PORT}`);
    console.log(`PUBLIC_KEY:  ${PUBLIC_KEY}`);
    console.log(`START_SEQNO: ${START_SEQNO}`);
    console.log(`END_SEQNO:   ${END_SEQNO}`);
    console.log(`TX_PAGE:     ${TX_PAGE}`);

    const client = createClient();
    let totalMcTx = 0;
    let totalShardTx = 0;
    let skippedBlocks = 0;
    const seenShards = new Set<string>();
    const shardStats = new Map<string, { blocks: number; tx: number }>();

    const startChainTime = await getMasterchainBlockTime(client, START_SEQNO - 1);
    const endChainTime = await getMasterchainBlockTime(client, END_SEQNO);

    for (let seqno = START_SEQNO; seqno <= END_SEQNO; seqno++) {
        console.log(`\n=== MASTERCHAIN ${seqno} ===`);
        try {
            const { mc, shard } = await processSeqno(client, seqno, seenShards, shardStats);
            console.log(`TOTAL TX in MC ${seqno}: ${mc} mc + ${shard} shard = ${mc + shard}`);
            totalMcTx += mc;
            totalShardTx += shard;
        } catch (e) {
            skippedBlocks++;
            console.warn(
                `skip MASTERCHAIN ${seqno} (not available yet) due to error:`,
                e instanceof Error ? e.message : e
            );
        }
    }

    const grandTotal = totalMcTx + totalShardTx;
    const chainSeconds = endChainTime - startChainTime;
    if (chainSeconds <= 0) {
        console.error(`❌ Invalid time range: endChainTime (${endChainTime}) - startChainTime (${startChainTime}) = ${chainSeconds}s`);
        console.error("   Cannot compute TPS with non-positive duration");
        process.exit(1);
    }

    console.log("\n======== PER-SHARD STATS ========");
    console.log("Shard                              Blocks    TX");
    console.log("─".repeat(55));
    const sorted = [...shardStats.entries()].sort((a, b) => b[1].tx - a[1].tx);
    for (const [shardKey, stat] of sorted) {
        console.log(
            `${shardKey.padEnd(35)}${String(stat.blocks).padStart(6)}  ${String(stat.tx).padStart(8)}`
        );
    }
    console.log("─".repeat(55));
    const totalBlocks = sorted.reduce((sum, [, s]) => sum + s.blocks, 0);
    console.log(
        `${"TOTAL:".padEnd(35)}${String(totalBlocks).padStart(6)}  ${String(grandTotal).padStart(8)}`
    );

    console.log("\n========== SUMMARY ==========");
    console.log("Network:            ", NETWORK_NAME);
    console.log("Liteserver:         ", LITESERVER_HOST);
    console.log("START_SEQNO:        ", START_SEQNO);
    console.log("END_SEQNO:          ", END_SEQNO);
    console.log("Scanned MC blocks:  ", END_SEQNO - START_SEQNO + 1);
    console.log("Skipped MC blocks:  ", skippedBlocks);
    console.log("MC transactions:    ", totalMcTx);
    console.log("Shard transactions: ", totalShardTx);
    console.log("Total transactions: ", grandTotal);
    console.log("startChainTime:     ", startChainTime, `(${new Date(startChainTime * 1000).toUTCString()})`);
    console.log("endChainTime:       ", endChainTime, `(${new Date(endChainTime * 1000).toUTCString()})`);
    const minutes = Math.floor(chainSeconds / 60);
    const seconds = chainSeconds % 60;
    console.log("Duration:           ", `${minutes}m ${seconds}s`);
    console.log("TPS:                ", (grandTotal / chainSeconds).toFixed(2));

    if (skippedBlocks > 0) {
        console.warn(`${skippedBlocks} MC blocks skipped`);
    }

    process.exit(0);
}

main().catch(console.error);