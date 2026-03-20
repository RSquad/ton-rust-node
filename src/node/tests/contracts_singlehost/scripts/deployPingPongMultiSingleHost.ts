import { Address, Cell } from '@ton/core';
import { PingPongMulti } from '../wrappers/PingPongMulti';
import fs from 'fs';
import { log } from 'console';
import { randomInt } from 'crypto';
import { deploySingleHost } from '../wrappers/SingleHostUtils';
import { getAccountState, sendMessage } from '../wrappers/ConsoleUtils';
import { loadUnit } from '@ton/sandbox/dist/config/config.tlb-gen';

async function deployOne(hex: string, messages_to_send: number, anotherShard: number|undefined = undefined) {
    let contract;
    while (true) {
        contract = PingPongMulti.createFromConfig(
            {
                messages_to_send: messages_to_send,
                salt: randomInt(0, 2 ** 32 - 1),
            },
            Cell.fromHex(hex),
        );
        if (anotherShard === undefined) {
            break;
        }
        if ((contract.address.hash[0] & 0xc0) != anotherShard) {
            break;
        }
        log(`Address ${contract.address} is in the same shard, retrying...`);
    }

    let address_str = `${contract.address.workChain}:${contract.address.hash.toString('hex')}`;
    log('Contract address:', address_str);
    fs.appendFileSync(`build/ping_pong_multi_addresses.address`, address_str + "\n");
    const deployMsg = await contract.getDeployMsg(contract.address);
    await deploySingleHost(deployMsg);

    return contract;
}

export async function run() {

    try {
        fs.unlinkSync(`build/ping_pong_multi_addresses.address`);
    } catch (error) { }

    const pairs_count = 5;
    const messages_in_one_wave = 5;
    const messages_to_send = Number.MAX_SAFE_INTEGER;

    for (let i = 0; i < pairs_count; i++) {

        log(`${i} Preparing address and deploy message...`);
        const data = fs.readFileSync('build/PingPongMulti.compiled.json', 'utf8');
        const json = JSON.parse(data);

        const pp1 = await deployOne(json.hex, messages_to_send);
        const pp2 = await deployOne(json.hex, messages_to_send, pp1.address.hash[0] & 0xc0);

        log(`${i} Deployed`);
    }

    const content = fs.readFileSync(`build/ping_pong_multi_addresses.address`, 'utf-8');
    const addresses = content.split(/\r?\n/); 
    for (let i = 0; i < pairs_count; i++) {

        const a1 = Address.parse(addresses[i * 2]);
        const a2 = Address.parse(addresses[i * 2 + 1]);

        const startMessage = await PingPongMulti.getStartMessage(a1, a2, messages_in_one_wave);
        await sendMessage(startMessage);

        log(`Sent start message ${addresses[i * 2]} <-> ${addresses[i * 2 + 1]}`);
    }
}

(async () => await run())();
