import { beginCell, Cell, external } from '@ton/core';
import { log } from 'console';
import { getAccountState, sendMessage } from '../wrappers/ConsoleUtils';
import fs from 'fs';

export async function run() {
    let address = fs.readFileSync('build/counter.address', 'utf8').trim();
    let account = await getAccountState(address);
    if (account == undefined) {
        log(`Account ${address} does not exist`);
        return;
    }
    if (account.storage.state.type != 'active' || account.storage.state.state.data == null) {
        log(`Account ${address} is not active`);
        return;
    }
    let cs = account.storage.state.state.data.beginParse();
    cs.loadUint(32);
    const last_call = cs.loadUint(32);

    const message = external({ 
                to: address,
                body: beginCell().storeUint(last_call, 32).endCell() 
            });
    await sendMessage(message);
}

(async () => await run())();
