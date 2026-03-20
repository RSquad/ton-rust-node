import { Address, toNano, beginCell, Cell, Contract, contractAddress, 
    ContractProvider, Sender, SendMode, external } from '@ton/core';

export type PingPongMultiConfig = {
    messages_to_send: number;
    salt: number;
};

export function pingPongMultiConfigToCell(config: PingPongMultiConfig): Cell {
    return beginCell()
        .storeUint(0, 64) // last_sent
        .storeUint(config.messages_to_send, 64)
        .storeUint(0, 32)   // last_call
        .storeUint(config.salt, 32)
        .storeUint(0, 1) // error
        .endCell();
}

export class PingPongMulti implements Contract {
    constructor(readonly address: Address, readonly init?: { code: Cell; data: Cell }) { }

    static createFromAddress(address: Address) {
        return new PingPongMulti(address);
    }

    static createFromConfig(config: PingPongMultiConfig, code: Cell, workchain = 0) {
        const data = pingPongMultiConfigToCell(config);
        const init = { code, data };
        return new PingPongMulti(contractAddress(workchain, init), init);
    }

    async sendDeploy(provider: ContractProvider, via: Sender, value: bigint) {
        await provider.internal(via, {
            value,
            sendMode: SendMode.PAY_GAS_SEPARATELY,
            body: beginCell().endCell(),
        });
    }

    async getExpected(provider: ContractProvider) {
        const result = await provider.get('get_expected', []);
        return result.stack.readNumber();
    }

    async getLastSent(provider: ContractProvider) {
        const result = await provider.get('get_last_sent', []);
        return result.stack.readNumber();
    }

    async sendStart(
        provider: ContractProvider,
        via: Sender,
        address: Address,
        wave = 1
    ) {
        await provider.internal(via, {
            value: toNano('1'),
            sendMode: SendMode.PAY_GAS_SEPARATELY,
            body: beginCell()
                .storeUint(1, 32)
                .storeAddress(address)
                .storeUint(wave, 8) 
                .endCell(),
        });
    }

    async getDeployMsg(address: Address) {
        return external({
            to: address,
            init: this.init,
            body: beginCell()
                .storeUint(0, 32)
                .storeUint(0, 32)
                .endCell()
        });
    }

    static async getStartMessage(address: Address, address2: Address, wave = 1) {
        return external({
            to: address,
            body: beginCell()
                .storeUint(1, 32)
                .storeUint(1, 32)
                .storeAddress(address2)
                .storeUint(wave, 8)
                .endCell(),
        });
    }
}
