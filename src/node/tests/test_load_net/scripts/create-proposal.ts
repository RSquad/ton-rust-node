import { Address, beginCell, Cell, Dictionary, fromNano, loadTransaction, toNano, TransactionDescriptionGeneric } from '@ton/core';
import * as fs from 'fs';
import { NetworkProvider, sleep } from "@ton/blueprint";
import { TonClient, WalletContractV3R2 } from '@ton/ton';
import { keyPairFromSecretKey } from '@ton/crypto';



export async function run(provider: NetworkProvider) {
    if (!process.env.MASTER_WALLET_KEY) {
        throw new Error('MASTER_WALLET_KEY is not set');
    }
    if (!fs.existsSync('config-params.json')) {
        throw new Error('config-params.json not found');
    }
    const params = JSON.parse(fs.readFileSync('config-params.json', 'utf8'));

    const keypair = keyPairFromSecretKey(Buffer.from(process.env.MASTER_WALLET_KEY!, 'hex'));
    const wallet = provider.open(WalletContractV3R2.create({
        workchain: -1,
        publicKey: keypair.publicKey,
        walletId: 42,
    }));
    console.log(`opened masterchain wallet: ${wallet.address.toString()}`);

    const now = Math.floor(Date.now() / 1000);
    const ui = provider.ui();
    const client = provider.api() as unknown as TonClient;
    const configAddress = Address.parse("-1:5555555555555555555555555555555555555555555555555555555555555555");
    console.log(`getting list of critical params...`);
    const cell10 = await getConfig(10);
    const criticals = cell10.beginParse().loadDictDirect(Dictionary.Keys.Uint(32), Dictionary.Values.Uint(0));
    console.log("get voting params...");
    const param11 = (await getConfig(11)).beginParse();
    param11.loadRef();
    const param11s = param11.loadRef().beginParse();
    param11s.loadUint(8);
    param11s.loadUint(8);
    param11s.loadUint(8);
    param11s.loadUint(8);
    param11s.loadUint(8);
    const minStoreSec = param11s.loadUint(32);
    console.log(`minStoreSec (for critical params only): ${minStoreSec}`);

    const expiresAt = now + parseInt(process.env.EXPIRES_IN_SECS ?? (await ui.input('Expires in seconds:')));

    for (const [name, param] of Object.entries(params)) {
        const id = parseInt(name.slice(1));
        const queryId = (now << 32) | id;
        console.log(`creating proposal for param ${name}...`);
        console.log(`param: ${JSON.stringify(param, null, 2)}`);
        let paramCell = (() => {
            switch (id) {
                case 15:
                    return buildConfigParam15(param);
                default:
                    throw new Error(`Unsupported param id: ${id}`);
            }
        })();

        console.log(`getting current config ${name}...`);
        const currentConfig = await getConfig(id);
        console.log(`current config hash: ${currentConfig.hash().toString('hex')}`);
        // create message body
        const bodyCell = beginCell()
            .storeUint(0x6e565052, 32)
            .storeUint(queryId, 64)
            .storeUint(expiresAt, 32)
            .storeRef(beginCell()
                .storeUint(0xf3, 8) // tag
                .storeUint(id, 32)
                .storeMaybeRef(paramCell)
                .storeBit(true)
                .storeBuffer(currentConfig.hash()) // replace with old hash
                .endCell())
            .storeBit(criticals.has(id))
            .endCell();

        console.log(`new proposal request (boc hex): ${bodyCell.toBoc().toString('hex')}`);
        console.log(`critical: ${criticals.has(id)}`);
        const { bits, refs } = calculateBitsAndRefs(paramCell);
        console.log(`proposal bits: ${bits}, refs: ${refs}`);
        const price = await proposalStoragePrice(
            provider,
            configAddress,
            criticals.has(id),
            expiresAt - now,
            bits + 1024, // from config-code.fc
            refs + 2, // from config-code.fc
        );
        if (price < 0) {
            throw new Error(`proposal expiresIn is les then minimum`);
        }
        console.log(`proposal price: ${fromNano(price)} TON`);

        console.log(`sending proposal...`);
        await wallet.sender(keypair.secretKey).send({
            to: configAddress,
            value: price + toNano('1'),
            body: bodyCell,
        });
        // Wait for transaction to be processed
        await sleep(3000);
        console.log(`proposal sent`);

        const state = await client.getContractState(configAddress);
        const { lt, hash } = state.lastTransaction!;
        const transactions = await getTransactions(configAddress, 10, parseInt(lt), hash);
        for (const txCell of transactions) {
            const tx = loadTransaction(Cell.fromBoc(Buffer.from(txCell.TransactionId.data, 'base64'))[0].beginParse());
            if (tx.inMessage! && tx.inMessage.info.src!.toString() === wallet.address.toString()) {
                const description = tx.description as TransactionDescriptionGeneric;
                console.log(`tx hash: ${tx.hash().toString('hex')}`);
                console.log(`aborted: ${description.aborted}`);
                const computePhase = description.computePhase;
                console.log(`compute phase: ${computePhase.type}`);
                if (computePhase.type === 'vm') {
                    console.log(`  success: ${computePhase.success}`);
                    console.log(`  gasUsed: ${computePhase.gasUsed}`);
                    console.log(`  exitCode: ${computePhase.exitCode}`);
                    console.log(`  vmSteps: ${computePhase.vmSteps}`);
                } else {
                    console.log(`  reason: ${computePhase.reason}`);
                }

                for (const [idx, msg] of tx.outMessages) {
                    const answerTag = msg.body.beginParse().loadUint(32);
                    console.log(`config-contract outbound message #${idx}: 0x${answerTag.toString(16)}`);
                    if (answerTag === 0xee565052) {
                        console.log('proposal accepted');
                    }
                }
                break;
            }
        }
    }
}

function calculateBitsAndRefs(paramCell: Cell): { bits: number, refs: number } {
    let bits = 0;
    let refs = 0;
    const visited = new Set<string>();
    const s = paramCell.beginParse();
    bits += s.remainingBits;
    const toVisit = paramCell.refs;

    for (const ref of toVisit) {
        if (!visited.has(ref.hash().toString('hex'))) {
            bits += ref.bits.length;
            refs += 1;
            visited.add(ref.hash().toString('hex'));
            toVisit.push(...ref.refs);
        }
    }
    return {
        bits,
        refs
    };
}

async function proposalStoragePrice(
    provider: NetworkProvider,
    configAddress: Address,
    isCritical: boolean,
    expiresIn: number,
    bits: number,
    refs: number
) {
    const contract = provider.provider(configAddress);
    let result = await contract.get("proposal_storage_price", [
        { type: "int", value: isCritical ? -1n : 0n },
        { type: "int", value: BigInt(expiresIn) },
        { type: "int", value: BigInt(bits) },
        { type: "int", value: BigInt(refs) }
    ]);

    return result.stack.readBigNumber();
}

async function getConfig(id: number): Promise<Cell> {
    try {
        const idx = process.argv.findIndex(arg => arg == '--custom');
        const url = idx !== -1 ? process.argv[idx + 1] : 'http://127.0.0.1:3301';
        const baseUrl = url.endsWith("jsonRPC") ? url.slice(0, -6) : url;
        const normalizedUrl = baseUrl.endsWith("/") ? baseUrl : `${baseUrl}/`;
        const result = await fetch(`${normalizedUrl}getConfigParam?config_id=${id}`, {
            method: 'GET',
            headers: {
                'Content-Type': 'application/json',
            },
        });
        const data = await result.json();
        if (!data.ok) {
            throw new Error(`Failed to get config: ${data.error}`);
        }
        return Cell.fromBase64(data.result.config.bytes as string);
    } catch (error) {
        console.error(`Failed to get config: ${error}`);
        throw error;
    }
}

async function getTransactions(address: Address, limit: number, lt: number, hash: string) {
    const result = await fetch(`http://127.0.0.1:3301/jsonRPC`, {
        method: 'POST',
        headers: {
            'Content-Type': 'application/json',
        },
        body: JSON.stringify({
            jsonrpc: '2.0',
            method: 'getTransactions',
            params: {
                address: address.toString(),
                limit,
                lt,
                hash,
            },
        }),
    });
    const data = await result.json();
    if (!data.ok) {
        throw new Error(`Failed to get transactions: ${data.error}`);
    }
    return data.result;
}

function buildConfigParam15(param: any) {
    return beginCell()
        .storeUint(param.validators_elected_for, 32)
        .storeUint(param.elections_start_before, 32)
        .storeUint(param.elections_end_before, 32)
        .storeUint(param.stake_held_for, 32)
        .endCell();
}