import { Cell } from '@ton/core';
import { Test } from '../wrappers/Counter';
import fs from 'fs';
import { log } from 'console';
import { randomInt } from 'crypto';
import { deploySingleHost } from '../wrappers/SingleHostUtils';

export async function run() {

    log('Preparing address and deploy message...');
    const data = fs.readFileSync('build/Counter.compiled.json', 'utf8');
    const json = JSON.parse(data);
    const counter = Test.createFromConfig(
        {
            count: 0,
            last_call: 0,
            salt: randomInt(0, 2 ** 32 - 1)  // Random salt for uniqueness
        },
        Cell.fromHex(json.hex),
    );
    let address_str = `${counter.address.workChain}:${counter.address.hash.toString('hex')}`;
    log('Contract address:', address_str);
    fs.writeFileSync('build/counter.address', address_str);
    const deployMsg = await counter.getDeployMsg(counter.address);

    deploySingleHost(deployMsg);
}

(async () => await run())();
