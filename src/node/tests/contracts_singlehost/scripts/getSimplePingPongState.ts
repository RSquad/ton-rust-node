import fs from 'fs';
import { log } from 'console';
import { getAccountState } from '../wrappers/ConsoleUtils';

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
    cs.loadUint(32);
    const acc = cs.loadUint(32);

    return acc;
}

export async function run() {
    const content = fs.readFileSync(`build/simple_ping_pong_addresses.address`, 'utf-8');
    const addresses = content.split(/\r?\n/); 
    var i = 0;
    for (const address of addresses) {
        if (address.trim() === '') continue; 
        const acc = await getAcc(address);
        log(`${i}  ${acc}`);
        i++;
    }
}

(async () => await run())();
