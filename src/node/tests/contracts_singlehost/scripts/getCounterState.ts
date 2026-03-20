import { log } from 'console';
import { getAccountState } from '../wrappers/ConsoleUtils';
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
    log(`number = ${cs.loadUint(32)}`);
    log(`last call = ${cs.loadUint(32)}`);
    log(`salt = ${cs.loadUint(32)}`);
}

(async () => await run())();
