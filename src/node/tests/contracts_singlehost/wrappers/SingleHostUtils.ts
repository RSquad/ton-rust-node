import { Address, Cell, Message } from '@ton/core';
import { log } from 'console';
import { askMoney } from './GiverUtils';
import { getAccountState, sendMessage } from './ConsoleUtils';

export async function deploySingleHost(deployMsg: Message) {

    if (!(deployMsg.info.dest instanceof Address)) {
        throw new Error('Invalid address type in deploy message');
    }
    const address_str = `${deployMsg.info.dest.workChain}:${deployMsg.info.dest.hash.toString('hex')}`;

    // Ask giver for money
    log('Asking giver for money...');
    await askMoney(address_str, 10000);

    // Wait for money
    log('Waiting for money...');
    await new Promise(resolve => setTimeout(resolve, 3000));
    let received = false;
    for (let attempt = 0; attempt < 20; attempt++) {
        const state = await getAccountState(address_str);
        if (state != undefined) {
            log(`Money received, account balance: ${state?.storage.balance.coins}`);
            received = true;
            break;
        }
        // Wait 1 second
        await new Promise(resolve => setTimeout(resolve, 3000));
        log(`Attempt ${attempt + 1}: still waiting for money...`);
    }
    if (!received) {
        throw new Error('Money not received');
    }

    // Send deploy message
    log('Sending deploy message...');
    await sendMessage(deployMsg);

    // Wait for deploy
    log('Waiting for deploy...');
    let deployed = false;
    await new Promise(resolve => setTimeout(resolve, 3000));
    for (let attempt = 0; attempt < 20; attempt++) {
        const state = await getAccountState(address_str);
        if (state != undefined) {
            if (state.storage.state.type === 'active' && state.storage.state.state.data != null) {
                log('Contract deployed');
                deployed = true;
                break;
            }
        }
        // Wait 1 second
        await new Promise(resolve => setTimeout(resolve, 3000));
        log(`Attempt ${attempt + 1}: still waiting for deploy...`);
    }
    if (!deployed) {
        throw new Error('Contract was not deployed');
    }
}
