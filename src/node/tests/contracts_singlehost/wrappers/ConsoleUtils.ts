import { exec } from 'child_process';
import fs from 'fs';
import { Cell, Message, beginCell, storeMessage, loadShardAccount, Account, loadAccount } from '@ton/core';
import { Maybe } from '@ton/core/src/utils/maybe';

export async function getAccountState(address: string): Promise<Maybe<Account>> {
    const boc_file = "build/acc_state.boc";
    try { fs.unlinkSync(boc_file); } catch (e) { }
    try { await callConsole(`getaccountstate ${address} ${boc_file}`) } catch (e) { return undefined; }
    if (!fs.existsSync(boc_file)) {
        return undefined;
    }
    const data = fs.readFileSync(boc_file);
    const root = Cell.fromBoc(data)[0];
    let cs = root.beginParse()
    if (!cs.loadBit()) {
        // uninit account
        return undefined;
    }
    const account = loadAccount(cs);
    fs.unlinkSync(boc_file);
    return account;
}

export async function sendMessage(msg: Message): Promise<void> {
    const msgBuilder = beginCell();
    storeMessage(msg)(msgBuilder);
    const msg_cell = msgBuilder.endCell();
    const msg_file = "build/msg.boc";
    const boc = msg_cell.toBoc();
    fs.writeFileSync(msg_file, boc);    
    await callConsole(`sendmessage ${msg_file}`);
    fs.unlinkSync(msg_file);
}

function callConsole(command: string) {
    console.log(`Calling node's console with command: ${command}`);
    const full_command = `../../../target/release/console -C ../test_run_net_py/tmp/node_1/console.json -c "${command}"`;
    return new Promise<string>((resolve, reject) => {
        exec(full_command, (error, stdout, stderr) => {
            if (error) {
                reject(error);
                return;
            }
            resolve(stdout);
        });
    });
}

