import { Cell } from '@ton/core';
import { PingPong } from '../wrappers/PingPong';
import fs from 'fs';
import { log } from 'console';
import { randomInt } from 'crypto';
import { deploySingleHost } from '../wrappers/SingleHostUtils';
import { getAccountState, sendMessage } from '../wrappers/ConsoleUtils';
import { loadUnit } from '@ton/sandbox/dist/config/config.tlb-gen';

async function deployOne(hex: string, i: number, anotherShard: number|undefined = undefined) {
    let pingPong;
    while (true) {
        pingPong = PingPong.createFromConfig(
            {
                accumulator: BigInt(0),
                last_call: 0,
                salt: randomInt(0, 2 ** 32 - 1),
                error: 0
            },
            Cell.fromHex(hex),
        );
        if (anotherShard === undefined) {
            break;
        }
        if ((pingPong.address.hash[0] & 0xc0) != anotherShard) {
            break;
        }
        log(`Address ${pingPong.address} is in the same shard, retrying...`);
    }

    let address_str = `${pingPong.address.workChain}:${pingPong.address.hash.toString('hex')}`;
    log('Contract address:', address_str);
    fs.appendFileSync(`build/ping_pong_addresses.address`, address_str + "\n");
    const deployMsg = await pingPong.getDeployMsg(pingPong.address);
    await deploySingleHost(deployMsg);

    return pingPong;
}

export async function run() {

    if (fs.existsSync(`build/ping_pong_addresses.address`)) {
        fs.unlinkSync(`build/ping_pong_addresses.address`);
    }

    for (let i = 0; i < 100; i++) {

        log(`${i} Preparing address and deploy message...`);
        const data = fs.readFileSync('build/PingPong.compiled.json', 'utf8');
        const json = JSON.parse(data);

        const pp1 = await deployOne(json.hex, 1);
        const pp2 = await deployOne(json.hex, 2, pp1.address.hash[0] & 0xc0);

        log(`${i} Deployed`);

        const startMessage = await pp1.getStartMessage(pp1.address, pp2.address);
        await sendMessage(startMessage);

        log(`${i} Sent start message`);
    }
}

(async () => await run())();
