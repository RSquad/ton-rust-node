import { Address, toNano, beginCell, Cell, Contract, contractAddress, 
    ContractProvider, Sender, SendMode, external } from '@ton/core';

export type PingPongConfig = {
    accumulator: bigint;
    last_call: number;
    salt: number;
    error: number;
};

export function pingPongConfigToCell(config: PingPongConfig): Cell {
    return beginCell()
        .storeUint(config.accumulator, 64)
        .storeUint(config.last_call, 32)
        .storeUint(config.salt, 32)
        .storeUint(config.error, 1)
        .endCell();
}

export class PingPong implements Contract {
    constructor(readonly address: Address, readonly init?: { code: Cell; data: Cell }) { }

    static createFromAddress(address: Address) {
        return new PingPong(address);
    }

    static createFromConfig(config: PingPongConfig, code: Cell, workchain = 0) {
        const data = pingPongConfigToCell(config);
        const init = { code, data };
        return new PingPong(contractAddress(workchain, init), init);
    }

    async sendDeploy(provider: ContractProvider, via: Sender, value: bigint) {
        await provider.internal(via, {
            value,
            sendMode: SendMode.PAY_GAS_SEPARATELY,
            body: beginCell().endCell(),
        });
    }

    async getAccumulator(provider: ContractProvider) {
        const result = await provider.get('get_accumulator', []);
        return result.stack.readBigNumber();
    }

    async getError(provider: ContractProvider) {
        const result = await provider.get('get_error', []);
        return result.stack.readNumber();
    }

    async sendStart(
        provider: ContractProvider,
        via: Sender,
        address: Address,
    ) {
        await provider.internal(via, {
            value: toNano('1'),
            sendMode: SendMode.PAY_GAS_SEPARATELY,
            body: beginCell()
                .storeUint(1, 32)
                .storeAddress(address)
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

    async getStartMessage(address: Address, address2: Address) {
        return external({
            to: address,
            body: beginCell()
                .storeUint(1, 32)
                .storeUint(1, 32)
                .storeAddress(address2)
                .endCell(),
        });
    }
}
