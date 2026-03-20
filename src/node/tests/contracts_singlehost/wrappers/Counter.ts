import {
    Address,
    beginCell,
    Cell,
    Contract,
    contractAddress,
    ContractProvider,
    external,
    Sender,
    SendMode,
} from '@ton/core';

export type TestConfig = {
    last_call: number;
    count: number;
    salt: number;
};

export function testConfigToCell(config: TestConfig): Cell {
    return beginCell().storeUint(config.last_call, 32).storeUint(config.count, 32).storeUint(config.salt, 32).endCell();
}

export class Test implements Contract {
    constructor(
        readonly address: Address,
        readonly init?: { code: Cell; data: Cell },
    ) {}

    static createFromAddress(address: Address) {
        return new Test(address);
    }

    static createFromConfig(config: TestConfig, code: Cell, workchain = 0) {
        const data = testConfigToCell(config);
        const init = { code, data };
        return new Test(contractAddress(workchain, init), init);
    }

    async sendDeploy(provider: ContractProvider, via: Sender, value: bigint) {
        await provider.internal(via, {
            value,
            sendMode: SendMode.PAY_GAS_SEPARATELY,
            body: beginCell().endCell(),
        });
    }

    async getDeployMsg(address: Address) {
        return external({ 
            to: address,
            init: this.init, 
            body: beginCell().storeUint(0, 32).endCell() 
        });
    }
}
