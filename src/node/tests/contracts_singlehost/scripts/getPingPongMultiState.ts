import { Cell, fromNano } from '@ton/core';
import { PingPong } from '../wrappers/PingPong';
import fs from 'fs';
import { log } from 'console';
import { randomInt } from 'crypto';
import { deploySingleHost } from '../wrappers/SingleHostUtils';
import { getAccountState, sendMessage } from '../wrappers/ConsoleUtils';
import { loadUnit } from '@ton/sandbox/dist/config/config.tlb-gen';

async function printAccState(address: string) {
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
    const last_sent = cs.loadUintBig(64);
    const expected = cs.loadUintBig(64);
    cs.loadUint(32);
    cs.loadUint(32);
    const error = cs.loadUint(1);
    if (error != 0) {
        log(`Error was set in account ${address}`);
    }
    log(`Last sent: ${last_sent.toString()}, Expected: ${expected.toString()}, Balance: ${fromNano(account.storage.balance.coins)}`);
}

export async function run() {
    const content = fs.readFileSync(`build/ping_pong_multi_addresses.address`, 'utf-8');
    const addresses = content.split(/\r?\n/);
    var i = 0;
    for (const address of addresses) {
        if (address.trim() === '') continue;
        log(`\n ${i} Address: ${address}`);
        const acc = await printAccState(address);
        i++;
    }
}

(async () => await run())();
