import { Address, toNano, beginCell, Cell, Contract, contractAddress, 
    ContractProvider, Sender, SendMode, external } from '@ton/core';

export type SimplePingPongConfig = {
    last_call: number;
    accumulator: number;
    salt: number;
};

export function simplePingPongConfigToCell(config: SimplePingPongConfig): Cell {
    return beginCell()
        .storeUint(config.last_call, 32)
        .storeUint(config.accumulator, 32)
        .storeUint(config.salt, 32)
        .endCell();
}

export class SimplePingPong implements Contract {
    constructor(readonly address: Address, readonly init?: { code: Cell; data: Cell }) { }

    static createFromAddress(address: Address) {
        return new SimplePingPong(address);
    }

    static createFromConfig(config: SimplePingPongConfig, code: Cell, workchain = 0) {
        const data = simplePingPongConfigToCell(config);
        const init = { code, data };
        return new SimplePingPong(contractAddress(workchain, init), init);
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
