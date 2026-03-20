import { Address, Cell } from '@ton/core';
import { SimplePingPong } from '../wrappers/SimplePingPong';
import fs from 'fs';
import { log } from 'console';
import { randomInt } from 'crypto';
import { deploySingleHost } from '../wrappers/SingleHostUtils';
import { getAccountState, sendMessage } from '../wrappers/ConsoleUtils';
import { loadUnit } from '@ton/sandbox/dist/config/config.tlb-gen';

async function deployOne(hex: string, i: number, anotherShard: number|undefined = undefined) {
    let simplePingPong;
    while (true) {
        simplePingPong = SimplePingPong.createFromConfig(
            {
                accumulator: 0,
                last_call: 0,
                salt: randomInt(0, 2 ** 32 - 1),
            },
            Cell.fromHex(hex),
        );
        if (anotherShard === undefined) {
            break;
        }
        if ((simplePingPong.address.hash[0] & 0xc0) != anotherShard) {
            break;
        }
        log(`Address ${simplePingPong.address} is in the same shard, retrying...`);
    }

    let address_str = `${simplePingPong.address.workChain}:${simplePingPong.address.hash.toString('hex')}`;
    log('Contract address:', address_str);
    fs.appendFileSync(`build/simple_ping_pong_addresses.address`, address_str + "\n");
    const deployMsg = await simplePingPong.getDeployMsg(simplePingPong.address);
    await deploySingleHost(deployMsg);

    return simplePingPong;
}

export async function run() {

    try {
        fs.unlinkSync(`build/simple_ping_pong_addresses.address`);
    } catch (error) { }

    const pairs_count = 5;
    const messages_in_one_wave = 5;

    for (let i = 0; i < pairs_count; i++) {

        log(`${i} Preparing address and deploy message...`);
        const data = fs.readFileSync('build/SimplePingPong.compiled.json', 'utf8');
        const json = JSON.parse(data);

        const pp1 = await deployOne(json.hex, 1);
        const pp2 = await deployOne(json.hex, 2, pp1.address.hash[0] & 0xc0);

        log(`${i} Deployed`);
    }

    const content = fs.readFileSync(`build/simple_ping_pong_addresses.address`, 'utf-8');
    const addresses = content.split(/\r?\n/); 
    for (let i = 0; i < pairs_count; i++) {

        const a1 = Address.parse(addresses[i * 2]);
        const a2 = Address.parse(addresses[i * 2 + 1]);

        const startMessage = await SimplePingPong.getStartMessage(a1, a2, messages_in_one_wave);
        await sendMessage(startMessage);

        log(`Sent start message ${addresses[i * 2]} <-> ${addresses[i * 2 + 1]}`);
    }
}

(async () => await run())();
