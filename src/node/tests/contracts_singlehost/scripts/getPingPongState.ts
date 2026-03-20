import { Cell } from '@ton/core';
import { PingPong } from '../wrappers/PingPong';
import fs from 'fs';
import { log } from 'console';
import { randomInt } from 'crypto';
import { deploySingleHost } from '../wrappers/SingleHostUtils';
import { getAccountState, sendMessage } from '../wrappers/ConsoleUtils';
import { loadUnit } from '@ton/sandbox/dist/config/config.tlb-gen';

async function getAcc(address: string) {
     let account = await getAccountState(address);
    if (account == undefined) {
        log(`Account ${address} does not exist`);
        return;
    }
    if (account.storage.state.type != 'active' || account.storage.state.state.data == null) {
        log(`Account ${address} is not active`);
        return;
    }

    var cs = account.storage.state.state.data.beginParse();
    const acc = cs.loadUintBig(64);
    cs.loadUint(32);
    cs.loadUint(32);
    const error = cs.loadUint(1);
    if (error != 0) {
        log(`Error was set in account ${address}`);
        return acc;
    }

    return acc;
}

export async function run() {
    const content = fs.readFileSync(`build/ping_pong_addresses.address`, 'utf-8');
    const addresses = content.split(/\r?\n/); 
    var i = 0;
    for (const address of addresses) {
        if (address.trim() === '') continue; 
        const acc = await getAcc(address);
        log(`${i}  ${acc?.toString(2)}`);
        i++;
    }
}

(async () => await run())();
